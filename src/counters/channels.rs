// SPDX-License-Identifier: GPL-3.0-or-later
use super::{
    Snapshot,
    config::{Metric, Targets},
    source::Stats,
};
use serenity::all::{ChannelId, ChannelType, EditChannel, Http};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

const RENAME_INTERVAL: Duration = Duration::from_secs(300);
#[derive(Default)]
enum RenameState {
    #[default]
    Fresh,
    Attempted {
        at: Instant,
        published: Option<String>,
    },
}
impl RenameState {
    fn begin(&mut self, name: &str, now: Instant) -> bool {
        let published = match self {
            Self::Fresh => None,
            Self::Attempted { at, published } => {
                if published.as_deref() == Some(name) || now.duration_since(*at) < RENAME_INTERVAL {
                    return false;
                }
                published.take()
            }
        };
        *self = Self::Attempted { at: now, published };
        true
    }
    fn succeeded(&mut self, name: String) {
        if let Self::Attempted { published, .. } = self {
            *published = Some(name);
        }
    }
}
struct Counter {
    id: ChannelId,
    metric: Metric,
    state: RenameState,
}
fn channel_name(stats: Option<Stats>, metric: Metric) -> String {
    match metric {
        Metric::Registered => match stats.and_then(|s| s.registered) {
            Some(n) => format!("👥 Registered: {n}"),
            None => "👥 Registered: unavailable".into(),
        },
        Metric::Online => match stats {
            Some(s) => format!("🟢 Online: {}", s.online),
            None => "🟡 Online: unavailable".into(),
        },
        Metric::Matches => match stats.and_then(|s| s.matches) {
            Some(n) => format!("🎮 Matches: {n}"),
            None => "🎮 Matches: unavailable".into(),
        },
        Metric::Queued => match stats.and_then(|s| s.queued) {
            Some(n) => format!("⏳ Queued: {n}"),
            None => "⏳ Queued: unavailable".into(),
        },
    }
}
impl Counter {
    async fn update(&mut self, http: &Http, snapshot: Snapshot) {
        if snapshot == Snapshot::Pending {
            return;
        }
        let name = channel_name(snapshot.stats(), self.metric);
        if !self.state.begin(&name, Instant::now()) {
            return;
        }
        let result = async {
            let channel = self
                .id
                .to_channel(http)
                .await
                .map_err(|_| "Cannot read counter channel")?
                .guild()
                .ok_or("Counter must be a server voice channel")?;
            if channel.kind != ChannelType::Voice {
                return Err("Counter must be a voice channel");
            }
            if channel.name != name {
                self.id
                    .edit(http, EditChannel::new().name(&name))
                    .await
                    .map_err(|_| "Cannot rename counter; check channel permissions")?;
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => self.state.succeeded(name),
            Err(reason) => eprintln!("{reason}"),
        }
    }
}
/// Discord rate-limit waits must not block HTTP polling or voice-room handling.
pub async fn refresh(http: Arc<Http>, targets: Targets, mut snapshots: watch::Receiver<Snapshot>) {
    let mut counters: Vec<_> = targets
        .into_entries()
        .map(|(id, metric)| Counter {
            id,
            metric,
            state: RenameState::default(),
        })
        .collect();
    if counters.is_empty() {
        return;
    }
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            result = snapshots.changed() => if result.is_err() { break; },
            _ = ticker.tick() => {},
        }
        for counter in &mut counters {
            let snapshot = *snapshots.borrow();
            counter.update(&http, snapshot).await;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_renames_retry_after_cooldown_successful_names_are_deduplicated() {
        let now = Instant::now();
        let mut state = RenameState::default();
        assert!(state.begin("Online: 10", now));
        assert!(!state.begin("Online: 10", now + Duration::from_secs(60)));
        assert!(state.begin("Online: 10", now + RENAME_INTERVAL));
        state.succeeded("Online: 10".into());
        assert!(!state.begin("Online: 10", now + RENAME_INTERVAL * 2));
        assert!(state.begin("Online: 12", now + RENAME_INTERVAL * 2));
        assert!(!state.begin("unavailable", now + RENAME_INTERVAL * 2));
        assert!(state.begin("unavailable", now + RENAME_INTERVAL * 3));
    }
    #[test]
    fn zero_is_not_an_outage() {
        assert_eq!(
            channel_name(
                Some(Stats {
                    online: 0,
                    registered: Some(30),
                    matches: None,
                    queued: None,
                }),
                Metric::Online
            ),
            "🟢 Online: 0"
        );
        assert_eq!(channel_name(None, Metric::Online), "🟡 Online: unavailable");
    }

    #[test]
    fn new_counters_distinguish_counts_missing_fields_and_outages() {
        for (matches, queued) in [(2, 1), (0, 0)] {
            let stats = Some(Stats {
                online: 14,
                registered: Some(72),
                matches: Some(matches),
                queued: Some(queued),
            });
            assert_eq!(
                channel_name(stats, Metric::Matches),
                format!("🎮 Matches: {matches}")
            );
            assert_eq!(
                channel_name(stats, Metric::Queued),
                format!("⏳ Queued: {queued}")
            );
        }
        let missing = Some(Stats {
            online: 0,
            registered: None,
            matches: None,
            queued: None,
        });
        for stats in [missing, None] {
            assert_eq!(
                channel_name(stats, Metric::Matches),
                "🎮 Matches: unavailable"
            );
            assert_eq!(
                channel_name(stats, Metric::Queued),
                "⏳ Queued: unavailable"
            );
        }
    }
}
