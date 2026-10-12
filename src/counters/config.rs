// SPDX-License-Identifier: GPL-3.0-or-later
use super::source::{Endpoint, Format, PollInterval, Source};
use crate::config::{ConfigResult, Environment};
use serenity::all::ChannelId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Online,
    Registered,
    Matches,
    Queued,
}

pub struct Targets(Vec<(ChannelId, Metric)>);
impl Targets {
    pub fn parse(
        online: Option<ChannelId>,
        registered: Option<ChannelId>,
        matches: Option<ChannelId>,
        queued: Option<ChannelId>,
    ) -> ConfigResult<Self> {
        let entries: Vec<_> = [
            (online, Metric::Online),
            (registered, Metric::Registered),
            (matches, Metric::Matches),
            (queued, Metric::Queued),
        ]
        .into_iter()
        .filter_map(|(id, metric)| id.map(|id| (id, metric)))
        .collect();
        for (i, (id, _)) in entries.iter().enumerate() {
            if entries[..i].iter().any(|(other, _)| other == id) {
                return Err("Counters must use different channels".into());
            }
        }
        Ok(Self(entries))
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
        let matches = env.optional("DISCORD_MATCHES_CHANNEL_ID")?;
        let queued = env.optional("DISCORD_QUEUED_CHANNEL_ID")?;
        let targets = Targets::parse(online, registered, matches, queued)?;
        let Some(endpoint) = env.optional::<Endpoint>("SRP_ONLINE_URL")? else {
            if targets.ids().next().is_some() {
                return Err("Counter channels require SRP_ONLINE_URL".into());
            }
            return Ok(None);
        };
        let interval = env
            .value("SRP_POLL_SECONDS")
            .unwrap_or_else(|| "10".into())
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
            targets,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_counter_targets_are_rejected() {
        for first in 0..4 {
            for second in first + 1..4 {
                let mut ids = [None; 4];
                ids[first] = Some(ChannelId::new(1));
                ids[second] = ids[first];
                assert!(Targets::parse(ids[0], ids[1], ids[2], ids[3]).is_err());
            }
        }
        let ids = [1, 2, 3, 4].map(|id| Some(ChannelId::new(id)));
        let targets = Targets::parse(ids[0], ids[1], ids[2], ids[3]).unwrap();
        assert_eq!(targets.ids().count(), 4);
        assert_eq!(
            targets
                .into_entries()
                .map(|(_, metric)| metric)
                .collect::<Vec<_>>(),
            [
                Metric::Online,
                Metric::Registered,
                Metric::Matches,
                Metric::Queued
            ]
        );
        assert!(Targets::parse(None, None, Some(ChannelId::new(1)), None).is_ok());
    }

    #[test]
    fn new_counters_require_a_source_and_default_to_ten_second_polling() {
        for key in ["DISCORD_MATCHES_CHANNEL_ID", "DISCORD_QUEUED_CHANNEL_ID"] {
            let env = Environment::new(|name| (name == key).then(|| "42".into()));
            assert!(Config::parse(&env).is_err());
            let env = Environment::new(|name| match name {
                "SRP_ONLINE_URL" => Some("https://example.com/api/stats".into()),
                name if name == key => Some("42".into()),
                _ => None,
            });
            let config = Config::parse(&env).unwrap().unwrap();
            assert_eq!(config.interval.duration().as_secs(), 10);
            assert_eq!(
                config.targets.ids().collect::<Vec<_>>(),
                [ChannelId::new(42)]
            );
        }
    }
}
