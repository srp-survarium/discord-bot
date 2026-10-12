// SPDX-License-Identifier: GPL-3.0-or-later
use super::config::Layout;
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, UserId};
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};

pub const ROOM_LIMIT: usize = 20;
pub const EMPTY_GRACE: Duration = Duration::from_secs(5);

/// Each room has exactly one occupancy state; there is no independent timer map.
#[derive(Clone, Copy, Debug)]
enum Occupancy {
    Occupied,
    EmptySince(Instant),
}
struct Room {
    channel: ChannelId,
    occupancy: Occupancy,
}

/// One room per owner, unique channel IDs, and no entry/category channel as a room.
/// Only the constructors and methods in this module can change these invariants.
pub struct RoomBook {
    layout: Layout,
    rooms: BTreeMap<UserId, Room>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    version: u8,
    layout: Layout,
    rooms: Vec<StoredRoom>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredRoom {
    channel: ChannelId,
    owner: UserId,
}

impl RoomBook {
    pub fn empty(layout: Layout) -> Self {
        Self {
            layout,
            rooms: BTreeMap::new(),
        }
    }
    pub fn parse(bytes: &[u8], expected: &Layout, now: Instant) -> Result<Self, &'static str> {
        let doc: Document =
            serde_json::from_slice(bytes).map_err(|_| "Invalid voice state document")?;
        if doc.version != 1 {
            return Err("Unsupported voice state version");
        }
        if &doc.layout != expected {
            return Err("Voice state belongs to a different layout");
        }
        let mut book = Self::empty(doc.layout);
        for room in doc.rooms {
            book.insert(room.owner, room.channel, now)?;
        }
        Ok(book)
    }
    pub fn document(&self) -> Document {
        Document {
            version: 1,
            layout: self.layout.clone(),
            rooms: self
                .rooms
                .iter()
                .map(|(owner, r)| StoredRoom {
                    owner: *owner,
                    channel: r.channel,
                })
                .collect(),
        }
    }
    pub fn existing(&self, owner: UserId) -> Option<ChannelId> {
        self.rooms.get(&owner).map(|r| r.channel)
    }
    pub fn is_full(&self) -> bool {
        self.rooms.len() >= ROOM_LIMIT
    }
    pub fn insert(
        &mut self,
        owner: UserId,
        channel: ChannelId,
        now: Instant,
    ) -> Result<(), &'static str> {
        if self.rooms.contains_key(&owner) {
            return Err("Duplicate room owner");
        }
        if self.layout.reserved(channel) || self.rooms.values().any(|r| r.channel == channel) {
            return Err("Reserved or duplicate room channel");
        }
        if self.is_full() {
            return Err("Temporary room capacity reached");
        }
        self.rooms.insert(
            owner,
            Room {
                channel,
                occupancy: Occupancy::EmptySince(now),
            },
        );
        Ok(())
    }
    pub fn forget(&mut self, channel: ChannelId) {
        self.rooms.retain(|_, r| r.channel != channel);
    }
    pub fn reset_grace(&mut self, now: Instant) {
        for room in self.rooms.values_mut() {
            room.occupancy = Occupancy::EmptySince(now);
        }
    }
    pub fn observe(&mut self, occupied: &HashSet<ChannelId>, now: Instant) -> Vec<ChannelId> {
        let mut expired = Vec::new();
        for room in self.rooms.values_mut() {
            room.occupancy = match (occupied.contains(&room.channel), room.occupancy) {
                (true, _) => Occupancy::Occupied,
                (false, Occupancy::Occupied) => Occupancy::EmptySince(now),
                (false, Occupancy::EmptySince(since)) => {
                    if now.duration_since(since) >= EMPTY_GRACE {
                        expired.push(room.channel);
                    }
                    Occupancy::EmptySince(since)
                }
            };
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout() -> Layout {
        serde_json::from_str(r#"{"guild":1,"category":2,"lobby":3}"#).unwrap()
    }
    #[test]
    fn rejects_invalid_persisted_ownership_at_the_boundary() {
        for rooms in [
            r#"[{"owner":0,"channel":4}]"#,
            r#"[{"owner":5,"channel":0}]"#,
            r#"[{"owner":5,"channel":3}]"#,
            r#"[{"owner":5,"channel":2}]"#,
            r#"[{"owner":5,"channel":4},{"owner":5,"channel":6}]"#,
            r#"[{"owner":5,"channel":4},{"owner":6,"channel":4}]"#,
        ] {
            let raw = format!(
                r#"{{"version":1,"layout":{{"guild":1,"category":2,"lobby":3}},"rooms":{rooms}}}"#
            );
            assert!(
                RoomBook::parse(raw.as_bytes(), &layout(), Instant::now()).is_err(),
                "{raw}"
            );
        }
    }
    #[test]
    fn occupied_rooms_survive_and_last_departure_starts_grace() {
        let now = Instant::now();
        let id = ChannelId::new(4);
        let mut book = RoomBook::empty(layout());
        book.insert(UserId::new(5), id, now).unwrap();
        assert!(
            book.observe(&HashSet::from([id]), now + EMPTY_GRACE)
                .is_empty()
        );
        assert!(
            book.observe(&HashSet::new(), now + EMPTY_GRACE * 2)
                .is_empty()
        );
        assert_eq!(
            book.observe(&HashSet::new(), now + EMPTY_GRACE * 3),
            vec![id]
        );
        book.observe(&HashSet::from([id]), now + EMPTY_GRACE * 3);
        assert!(
            book.observe(&HashSet::new(), now + EMPTY_GRACE * 4)
                .is_empty()
        );
    }
    #[test]
    fn restart_preserves_owner_and_resets_deadline() {
        let mut book = RoomBook::empty(layout());
        book.insert(UserId::new(5), ChannelId::new(4), Instant::now())
            .unwrap();
        let bytes = serde_json::to_vec(&book.document()).unwrap();
        let now = Instant::now();
        let mut loaded = RoomBook::parse(&bytes, &layout(), now).unwrap();
        assert_eq!(loaded.existing(UserId::new(5)), Some(ChannelId::new(4)));
        assert!(loaded.observe(&HashSet::new(), now).is_empty());
        loaded.forget(ChannelId::new(4));
        assert_eq!(loaded.existing(UserId::new(5)), None);
    }
}
