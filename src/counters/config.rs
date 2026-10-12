// SPDX-License-Identifier: GPL-3.0-or-later
use super::source::{Endpoint, Format, PollInterval, Source};
use crate::config::{ConfigResult, Environment};
use serenity::all::ChannelId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Online,
    Registered,
}

pub struct Targets(Vec<(ChannelId, Metric)>);
impl Targets {
    pub fn parse(online: Option<ChannelId>, registered: Option<ChannelId>) -> ConfigResult<Self> {
        if online.is_some() && online == registered {
            return Err("Counters must use different channels".into());
        }
        Ok(Self(
            [(online, Metric::Online), (registered, Metric::Registered)]
                .into_iter()
                .filter_map(|(id, metric)| id.map(|id| (id, metric)))
                .collect(),
        ))
    }
    pub fn into_entries(self) -> impl Iterator<Item = (ChannelId, Metric)> {
        self.0.into_iter()
    }
    pub fn ids(&self) -> impl Iterator<Item = ChannelId> + '_ {
        self.0.iter().map(|(id, _)| *id)
    }
}
pub struct Config {
    pub source: Source,
    pub interval: PollInterval,
    pub targets: Targets,
}
impl Config {
    pub fn parse<F: Fn(&str) -> Option<String>>(
        env: &Environment<F>,
    ) -> ConfigResult<Option<Self>> {
        let online = env.optional("DISCORD_ONLINE_CHANNEL_ID")?;
        let registered = env.optional("DISCORD_REGISTERED_CHANNEL_ID")?;
        let Some(endpoint) = env.optional::<Endpoint>("SRP_ONLINE_URL")? else {
            if online.is_some() || registered.is_some() {
                return Err("Counter channels require SRP_ONLINE_URL".into());
            }
            return Ok(None);
        };
        let interval = env
            .value("SRP_POLL_SECONDS")
            .unwrap_or_else(|| "60".into())
            .parse()?;
        let format = env
            .value("SRP_SOURCE_FORMAT")
            .unwrap_or_else(|| "json".into())
            .parse::<Format>()?;
        let token = env.value("SRP_ACCOUNT_TOKEN");
        let source = Source::new(endpoint, token.as_deref(), format)?;
        Ok(Some(Self {
            source,
            interval,
            targets: Targets::parse(online, registered)?,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_counter_targets_are_rejected() {
        let id = ChannelId::new(1);
        assert!(Targets::parse(Some(id), Some(id)).is_err());
        assert!(Targets::parse(Some(id), None).is_ok());
    }
}
