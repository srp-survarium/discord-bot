// SPDX-License-Identifier: GPL-3.0-or-later
use serenity::all::{
    AuditLogEntry, UserId,
    audit_log::{Action, MemberAction},
};
use std::collections::{HashMap, HashSet};

const HOUR: i64 = 3600;
pub const DAY: i64 = 24 * HOUR;
const BURST: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    Daily,
    Hourly,
    Burst,
}
impl Violation {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Daily => {
                "Moderation guard: more than 15 people kicked/banned in 24 hours; manual role restoration required"
            }
            Self::Hourly => {
                "Moderation guard: more than 10 people kicked/banned in one hour; manual role restoration required"
            }
            Self::Burst => {
                "Moderation guard: 3 people kicked/banned in 30 seconds; manual role restoration required"
            }
        }
    }
}

/// Only attributed kicks/bans enter the policy. Ordinary departures, unbans,
/// and incomplete audit entries cannot implicate a moderator.
pub struct Removal {
    actor: UserId,
    target: UserId,
    at: i64,
}
impl Removal {
    pub fn from_entry(entry: &AuditLogEntry) -> Option<Self> {
        if !matches!(
            entry.action,
            Action::Member(MemberAction::Kick | MemberAction::BanAdd)
        ) {
            return None;
        }
        Some(Self {
            actor: entry.user_id,
            target: UserId::new(entry.target_id?.get()),
            at: entry.id.created_at().unix_timestamp(),
        })
    }
}

#[derive(Default)]
struct Counts {
    daily: HashSet<UserId>,
    hourly: HashSet<UserId>,
    burst: HashSet<UserId>,
}
pub struct Window {
    now: i64,
    actors: HashMap<UserId, Counts>,
}
impl Window {
    pub fn new(now: i64) -> Self {
        Self {
            now,
            actors: HashMap::new(),
        }
    }
    pub fn record(&mut self, removal: Removal) -> Option<(UserId, Violation)> {
        if removal.at <= self.now - DAY || removal.at > self.now {
            return None;
        }
        let counts = self.actors.entry(removal.actor).or_default();
        // Bounded sets: further victims do not change an already established breach.
        if counts.daily.len() < 16 {
            counts.daily.insert(removal.target);
        }
        if removal.at > self.now - HOUR && counts.hourly.len() < 11 {
            counts.hourly.insert(removal.target);
        }
        if removal.at > self.now - BURST && counts.burst.len() < 3 {
            counts.burst.insert(removal.target);
        }
        let violation = if counts.burst.len() >= 3 {
            Violation::Burst
        } else if counts.hourly.len() > 10 {
            Violation::Hourly
        } else if counts.daily.len() > 15 {
            Violation::Daily
        } else {
            return None;
        };
        Some((removal.actor, violation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn removal(actor: u64, target: u64, at: i64) -> Removal {
        Removal {
            actor: UserId::new(actor),
            target: UserId::new(target),
            at,
        }
    }
    #[test]
    fn daily_limit_counts_distinct_people_per_actor_without_inflating_shorter_windows() {
        let mut window = Window::new(100_000);
        for target in 1..=15 {
            for _ in 0..3 {
                assert_eq!(window.record(removal(1, target, 90_000)), None);
                assert_eq!(window.record(removal(2, target, 90_000)), None);
            }
        }
        assert_eq!(
            window.record(removal(1, 16, 95_000)),
            Some((UserId::new(1), Violation::Daily))
        );
        assert_eq!(window.record(removal(2, 15, 99_999)), None);
    }
    #[test]
    fn daily_boundary_is_exclusive_and_future_events_are_ignored() {
        let mut window = Window::new(100_000);
        for target in 1..=16 {
            assert_eq!(window.record(removal(1, target, 100_000 - DAY)), None);
            assert_eq!(window.record(removal(1, target, 100_001)), None);
        }
        for target in 1..=15 {
            assert_eq!(window.record(removal(1, target, 100_001 - DAY)), None);
        }
        assert_eq!(
            window.record(removal(1, 16, 100_001 - DAY)),
            Some((UserId::new(1), Violation::Daily))
        );
    }
    #[test]
    fn eleventh_distinct_person_triggers_hourly_limit_and_actors_are_separate() {
        let mut window = Window::new(10_000);
        for target in 1..=10 {
            assert_eq!(window.record(removal(1, target, 9_000)), None);
            assert_eq!(window.record(removal(2, target, 9_000)), None);
        }
        assert_eq!(
            window.record(removal(1, 11, 9_000)),
            Some((UserId::new(1), Violation::Hourly))
        );
    }
    #[test]
    fn third_distinct_person_in_thirty_seconds_triggers_burst() {
        let mut window = Window::new(10_000);
        assert_eq!(window.record(removal(1, 1, 9_971)), None);
        assert_eq!(window.record(removal(1, 2, 9_999)), None);
        assert_eq!(
            window.record(removal(1, 3, 10_000)),
            Some((UserId::new(1), Violation::Burst))
        );
    }
    #[test]
    fn replayed_or_repeated_targets_do_not_inflate_counts() {
        let mut window = Window::new(10_000);
        for _ in 0..30 {
            assert_eq!(window.record(removal(1, 1, 9_999)), None);
        }
        // A repeated target may have a newer event inside the burst window.
        assert_eq!(window.record(removal(1, 2, 9_000)), None);
        assert_eq!(window.record(removal(1, 2, 9_999)), None);
        assert_eq!(
            window.record(removal(1, 3, 9_999)),
            Some((UserId::new(1), Violation::Burst))
        );
    }
    #[test]
    fn rolling_boundaries_and_future_entries_do_not_count() {
        let mut window = Window::new(10_000);
        for target in 1..=11 {
            assert_eq!(window.record(removal(1, target, 6_400)), None);
            assert_eq!(window.record(removal(1, target, 10_001)), None);
        }
        for target in 1..=3 {
            assert_eq!(window.record(removal(1, target, 9_970)), None);
        }
    }
    #[test]
    fn audit_parser_ignores_non_removals_and_missing_targets() {
        let entry = |action: u8, target: Option<&str>| {
            serde_json::from_value::<AuditLogEntry>(serde_json::json!({
                "id": "1000000000000000000", "user_id": "1", "target_id": target,
                "action_type": action, "reason": null, "changes": null, "options": null
            }))
            .unwrap()
        };
        assert!(Removal::from_entry(&entry(20, Some("2"))).is_some());
        assert!(Removal::from_entry(&entry(22, Some("2"))).is_some());
        for action in [21, 23, 24, 25, 72] {
            assert!(Removal::from_entry(&entry(action, Some("2"))).is_none());
        }
        assert!(Removal::from_entry(&entry(22, None)).is_none());
    }
}
