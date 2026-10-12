// SPDX-License-Identifier: GPL-3.0-or-later
use crate::config::{ConfigResult, Environment};
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, ChannelType, GuildChannel, GuildId, Http, Permissions, UserId};
use std::path::PathBuf;

/// A complete local configuration. Partial configuration cannot enable the feature.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "RawLayout")]
pub struct Layout {
    guild: GuildId,
    category: ChannelId,
    lobby: ChannelId,
}

#[derive(Deserialize)]
struct RawLayout {
    guild: GuildId,
    category: ChannelId,
    lobby: ChannelId,
}
impl TryFrom<RawLayout> for Layout {
    type Error = &'static str;
    fn try_from(raw: RawLayout) -> Result<Self, Self::Error> {
        if raw.category == raw.lobby {
            return Err("entry channel and category must be different");
        }
        Ok(Self {
            guild: raw.guild,
            category: raw.category,
            lobby: raw.lobby,
        })
    }
}
impl Layout {
    pub fn guild(&self) -> GuildId {
        self.guild
    }
    pub fn category(&self) -> ChannelId {
        self.category
    }
    pub fn lobby(&self) -> ChannelId {
        self.lobby
    }
    pub fn is_room(&self, channel: &GuildChannel) -> bool {
        channel.guild_id == self.guild
            && channel.kind == ChannelType::Voice
            && channel.parent_id == Some(self.category)
            && channel.id != self.lobby
    }
    pub fn reserved(&self, id: ChannelId) -> bool {
        id == self.category || id == self.lobby
    }
}

pub struct Config {
    pub layout: Layout,
    pub state_path: PathBuf,
}
impl Config {
    pub fn parse<F: Fn(&str) -> Option<String>>(
        env: &Environment<F>,
    ) -> ConfigResult<Option<Self>> {
        let guild = env.optional("DISCORD_VOICE_GUILD_ID")?;
        let category = env.optional("DISCORD_VOICE_CATEGORY_ID")?;
        let lobby = env.optional("DISCORD_VOICE_LOBBY_CHANNEL_ID")?;
        let layout = match (guild, category, lobby) {
            (None, None, None) => return Ok(None),
            (Some(guild), Some(category), Some(lobby)) => Layout::try_from(RawLayout {
                guild,
                category,
                lobby,
            })?,
            _ => {
                return Err(
                    "Set all three DISCORD_VOICE_*_ID settings to enable voice rooms".into(),
                );
            }
        };
        Ok(Some(Self {
            layout,
            state_path: env
                .value("DISCORD_VOICE_STATE_PATH")
                .unwrap_or_else(|| ".rooms.json".into())
                .into(),
        }))
    }
}

/// Constructed only after Discord confirms channel types, hierarchy, and permissions.
/// This is a point-in-time capability: Discord may revoke it, so API failures still matter.
#[derive(Clone)]
pub struct ReadyLayout(Layout);
impl ReadyLayout {
    #[cfg(test)]
    pub fn fixture(layout: Layout) -> Self {
        Self(layout)
    }
    pub async fn resolve(layout: &Layout, http: &Http, bot: UserId) -> Result<Self, &'static str> {
        let channels = layout
            .guild
            .channels(http)
            .await
            .map_err(|_| "Cannot read voice channels")?;
        let lobby = channels
            .get(&layout.lobby)
            .ok_or("Entry channel is missing")?;
        let category = channels
            .get(&layout.category)
            .ok_or("Voice category is missing")?;
        if !layout.is_entry(lobby, category) {
            return Err("Entry voice channel must be inside the configured category");
        }
        let guild = layout
            .guild
            .to_partial_guild(http)
            .await
            .map_err(|_| "Cannot read server roles")?;
        let member = layout
            .guild
            .member(http, bot)
            .await
            .map_err(|_| "Cannot read bot permissions")?;
        let required = Permissions::VIEW_CHANNEL
            | Permissions::CONNECT
            | Permissions::MANAGE_CHANNELS
            | Permissions::MOVE_MEMBERS;
        if [lobby, category]
            .into_iter()
            .any(|c| !guild.user_permissions_in(c, &member).contains(required))
        {
            return Err(
                "Voice rooms need View Channel, Connect, Manage Channels and Move Members on the entry channel and category",
            );
        }
        Ok(Self(layout.clone()))
    }
    pub fn layout(&self) -> &Layout {
        &self.0
    }
}
impl Layout {
    fn is_entry(&self, lobby: &GuildChannel, category: &GuildChannel) -> bool {
        lobby.guild_id == self.guild
            && category.guild_id == self.guild
            && lobby.kind == ChannelType::Voice
            && category.kind == ChannelType::Category
            && lobby.parent_id == Some(self.category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_complete_configs_and_rejects_partial_zero_or_aliases() {
        for values in [["1", "2", "3"], ["", "", ""]] {
            let env = Environment::new(|key: &str| match key {
                "DISCORD_VOICE_GUILD_ID" => Some(values[0].into()),
                "DISCORD_VOICE_CATEGORY_ID" => Some(values[1].into()),
                "DISCORD_VOICE_LOBBY_CHANNEL_ID" => Some(values[2].into()),
                _ => None,
            });
            assert!(Config::parse(&env).is_ok());
        }
        for values in [["1", "", "3"], ["0", "2", "3"], ["1", "2", "2"]] {
            let env = Environment::new(|key: &str| match key {
                "DISCORD_VOICE_GUILD_ID" => Some(values[0].into()),
                "DISCORD_VOICE_CATEGORY_ID" => Some(values[1].into()),
                "DISCORD_VOICE_LOBBY_CHANNEL_ID" => Some(values[2].into()),
                _ => None,
            });
            assert!(Config::parse(&env).is_err());
        }
    }
}
