// SPDX-License-Identifier: GPL-3.0-or-later
mod config;
mod window;

pub use config::Config;
use serenity::all::{
    Http, Member, PartialGuild, Permissions, Role, UserId,
    audit_log::{Action, MemberAction},
};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use window::{DAY, Removal, Violation, Window};

struct Ready {
    guild: PartialGuild,
    bot: UserId,
}

fn validate_role(
    role: &Role,
    bot_top: &Role,
    permissions: Permissions,
) -> Result<(), &'static str> {
    if !permissions.administrator()
        && !permissions.contains(Permissions::MANAGE_ROLES | Permissions::VIEW_AUDIT_LOG)
    {
        return Err("Moderation guard needs Manage Roles and View Audit Log");
    }
    if role.managed || role >= bot_top {
        return Err("Moderation guard role must be unmanaged and below the bot's highest role");
    }
    if !role
        .permissions
        .intersects(Permissions::BAN_MEMBERS | Permissions::KICK_MEMBERS)
    {
        return Err("Moderation guard role must grant Ban Members or Kick Members");
    }
    Ok(())
}

fn role_permissions(guild: &PartialGuild, member: &Member) -> Permissions {
    guild
        .roles
        .values()
        .filter(|role| role.id.get() == guild.id.get() || member.roles.contains(&role.id))
        .fold(Permissions::empty(), |permissions, role| {
            permissions | role.permissions
        })
}

impl Ready {
    async fn resolve(config: &Config, http: &Http) -> Result<Self, &'static str> {
        let guild = config
            .guild()
            .to_partial_guild(http)
            .await
            .map_err(|_| "Cannot read moderation guard guild")?;
        let bot = http
            .get_current_user()
            .await
            .map_err(|_| "Cannot identify moderation guard bot")?
            .id;
        let member = config
            .guild()
            .member(http, bot)
            .await
            .map_err(|_| "Cannot read moderation guard bot permissions")?;
        let role = guild
            .roles
            .get(&config.role())
            .ok_or("Moderation guard role is missing")?;
        let top = member
            .roles
            .iter()
            .filter_map(|id| guild.roles.get(id))
            .max()
            .ok_or("Moderation guard bot needs a role above the guarded role")?;
        validate_role(role, top, role_permissions(&guild, &member))?;
        Ok(Self { guild, bot })
    }
}

pub async fn check(config: &Config, http: &Http) -> Result<(), &'static str> {
    Ready::resolve(config, http).await?;
    config
        .guild()
        .audit_logs(
            http,
            Some(Action::Member(MemberAction::BanAdd)),
            None,
            None,
            Some(1),
        )
        .await
        .map_err(|_| "Cannot read moderation audit log")?;
    Ok(())
}

async fn suspend(
    config: &Config,
    http: &Http,
    actor: UserId,
    violation: Violation,
) -> Result<(), &'static str> {
    // Refresh permissions/hierarchy before a mutation; Discord also enforces them.
    let ready = Ready::resolve(config, http).await?;
    if actor == ready.bot || actor == ready.guild.owner_id {
        return Ok(());
    }
    let member = match config.guild().member(http, actor).await {
        Ok(member) => member,
        Err(serenity::Error::Http(error))
            if error.status_code().is_some_and(|s| s.as_u16() == 404) =>
        {
            return Ok(());
        }
        Err(_) => return Err("Cannot read moderation guard target"),
    };
    if !member.roles.contains(&config.role()) {
        return Ok(());
    }
    http.remove_member_role(
        config.guild(),
        actor,
        config.role(),
        Some(violation.reason()),
    )
    .await
    .map_err(|_| "Cannot remove guarded role; check permissions and hierarchy")?;
    eprintln!("{}", violation.reason());
    // A successful HTTP response is not proof that another role grants no access.
    let member = config
        .guild()
        .member(http, actor)
        .await
        .map_err(|_| "Cannot verify moderator role removal")?;
    if member.roles.contains(&config.role()) {
        return Err("Guarded role is still present after removal");
    }
    if role_permissions(&ready.guild, &member).intersects(
        Permissions::ADMINISTRATOR | Permissions::BAN_MEMBERS | Permissions::KICK_MEMBERS,
    ) {
        return Err("Guarded role removed, but other roles still grant moderation permissions");
    }
    Ok(())
}

async fn scan(config: &Config, http: &Http) -> Result<(), &'static str> {
    let ready = Ready::resolve(config, http).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System clock is invalid")?
        .as_secs() as i64;
    let mut window = Window::new(now);
    let mut attempted = HashSet::new();
    let mut failure = None;
    for action in [MemberAction::Kick, MemberAction::BanAdd] {
        let mut before = None;
        // Bound work on extremely busy servers; already proven breaches are acted
        // on while scanning, before any later page can fail or hit this bound.
        for page in 0..100 {
            let logs = config
                .guild()
                .audit_logs(http, Some(Action::Member(action)), None, before, Some(100))
                .await
                .map_err(|_| "Cannot read moderation audit log")?;
            let Some(oldest) = logs.entries.iter().map(|entry| entry.id).min() else {
                break;
            };
            if before.is_some_and(|previous| oldest >= previous) {
                return Err("Moderation audit pagination did not advance");
            }
            for entry in &logs.entries {
                let Some(removal) = Removal::from_entry(entry) else {
                    continue;
                };
                if let Some((actor, violation)) = window.record(removal)
                    && actor != ready.bot
                    && actor != ready.guild.owner_id
                    && attempted.insert(actor)
                    && let Err(reason) = suspend(config, http, actor, violation).await
                {
                    failure.get_or_insert(reason);
                }
            }
            if logs.entries.len() < 100 || oldest.created_at().unix_timestamp() <= now - DAY {
                break;
            }
            before = Some(oldest);
            if page == 99 {
                failure.get_or_insert("Moderation audit scan exceeded its page limit");
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod http_tests;

pub fn start(config: Config, http: Arc<Http>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = None;
        loop {
            ticker.tick().await;
            let result = tokio::time::timeout(Duration::from_secs(60), scan(&config, &http))
                .await
                .unwrap_or(Err("Moderation guard scan timed out"));
            if previous != Some(result) {
                match result {
                    Ok(()) => eprintln!("Moderation guard active"),
                    Err(reason) => eprintln!("{reason}"),
                }
                previous = Some(result);
            }
            if result.is_err() {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_and_hierarchy_failures_cannot_enable_role_removal() {
        let mut role = Role::default();
        role.id = serenity::all::RoleId::new(2);
        role.position = 1;
        role.permissions = Permissions::BAN_MEMBERS;
        let mut top = Role::default();
        top.id = serenity::all::RoleId::new(3);
        top.position = 2;
        let required = Permissions::MANAGE_ROLES | Permissions::VIEW_AUDIT_LOG;
        assert!(validate_role(&role, &top, required).is_ok());
        assert!(validate_role(&role, &top, Permissions::MANAGE_ROLES).is_err());
        assert!(validate_role(&role, &top, Permissions::VIEW_AUDIT_LOG).is_err());
        assert!(validate_role(&role, &role, required).is_err());
        role.managed = true;
        assert!(validate_role(&role, &top, required).is_err());
        role.managed = false;
        role.permissions = Permissions::empty();
        assert!(validate_role(&role, &top, required).is_err());
    }
}
