// SPDX-License-Identifier: GPL-3.0-or-later
use crate::config::{ConfigResult, Environment};
use serenity::all::{GuildId, RoleId};

pub struct Config {
    guild: GuildId,
    role: RoleId,
}
impl Config {
    pub fn parse<F: Fn(&str) -> Option<String>>(
        env: &Environment<F>,
    ) -> ConfigResult<Option<Self>> {
        match (
            env.optional::<GuildId>("DISCORD_GUARD_GUILD_ID")?,
            env.optional::<RoleId>("DISCORD_GUARD_ROLE_ID")?,
        ) {
            (None, None) => Ok(None),
            (Some(guild), Some(role)) if guild.get() != role.get() => {
                Ok(Some(Self { guild, role }))
            }
            (Some(_), Some(_)) => Err("Moderation guard cannot remove @everyone".into()),
            _ => Err("Set both DISCORD_GUARD_GUILD_ID and DISCORD_GUARD_ROLE_ID".into()),
        }
    }
    pub fn guild(&self) -> GuildId {
        self.guild
    }
    pub fn role(&self) -> RoleId {
        self.role
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_configuration_must_be_complete_and_cannot_target_everyone() {
        let read = |guild: Option<&str>, role: Option<&str>| {
            Config::parse(&Environment::new(|key| match key {
                "DISCORD_GUARD_GUILD_ID" => guild.map(str::to_owned),
                "DISCORD_GUARD_ROLE_ID" => role.map(str::to_owned),
                _ => None,
            }))
        };
        assert!(read(None, None).unwrap().is_none());
        assert!(read(Some("1"), None).is_err());
        assert!(read(None, Some("2")).is_err());
        assert!(read(Some("1"), Some("1")).is_err());
        assert!(read(Some("invalid"), Some("2")).is_err());
        assert!(read(Some("1"), Some("2")).unwrap().is_some());
    }
}
