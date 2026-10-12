// SPDX-License-Identifier: GPL-3.0-or-later
mod channels;
mod config;
mod source;
pub use config::Config;
use serenity::all::{ActivityData, Context, Http, OnlineStatus};
use source::{Stats, activity_name};
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};

/// No ambiguous nested options: an initial request, an error, and a valid zero differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Snapshot {
    #[default]
    Pending,
    Unavailable,
    Available(Stats),
}
impl Snapshot {
    fn stats(self) -> Option<Stats> {
        match self {
            Self::Available(stats) => Some(stats),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Presence {
    published: Option<Snapshot>,
}
impl Presence {
    fn changed(&mut self, snapshot: Snapshot, reconnected: bool) -> bool {
        if !reconnected && self.published == Some(snapshot) {
            return false;
        }
        self.published = Some(snapshot);
        true
    }
}

pub fn start(
    config: Config,
    http: Arc<Http>,
    mut gateway: watch::Receiver<Option<Context>>,
) -> Vec<JoinHandle<()>> {
    let (sender, receiver) = watch::channel(Snapshot::Pending);
    let rename = tokio::spawn(channels::refresh(http, config.targets, receiver));
    let poll = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.interval.duration());
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut presence = Presence::default();
        let mut last = Snapshot::Pending;
        loop {
            let reconnected = tokio::select! {
                result = gateway.changed() => { if result.is_err() { break; } true },
                _ = ticker.tick() => false,
            };
            let Some(ctx) = gateway.borrow().clone() else {
                continue;
            };
            let snapshot = match config.source.fetch().await {
                Ok(stats) => {
                    if last == Snapshot::Unavailable {
                        eprintln!("Counter source recovered");
                    }
                    Snapshot::Available(stats)
                }
                Err(reason) => {
                    if last != Snapshot::Unavailable {
                        eprintln!("Counter source unavailable: {reason}");
                    }
                    Snapshot::Unavailable
                }
            };
            sender.send_replace(snapshot);
            if presence.changed(snapshot, reconnected) {
                ctx.set_presence(
                    Some(ActivityData::watching(activity_name(
                        snapshot.stats().map(|s| s.online),
                    ))),
                    if matches!(snapshot, Snapshot::Available(_)) {
                        OnlineStatus::Online
                    } else {
                        OnlineStatus::Idle
                    },
                );
            }
            last = snapshot;
        }
    });
    vec![rename, poll]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_recovery_reconnect_and_zero_are_distinct() {
        let mut presence = Presence::default();
        let zero = Snapshot::Available(Stats {
            online: 0,
            registered: None,
        });
        assert!(presence.changed(zero, false));
        assert!(!presence.changed(zero, false));
        assert!(presence.changed(Snapshot::Unavailable, false));
        assert!(!presence.changed(Snapshot::Unavailable, false));
        assert!(presence.changed(zero, false));
        assert!(presence.changed(zero, true));
        assert_eq!(activity_name(Some(0)), "0 players online");
    }
}
