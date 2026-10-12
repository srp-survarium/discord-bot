// SPDX-License-Identifier: GPL-3.0-or-later
mod config;
mod session;
mod state;
mod store;

pub use config::Config;
use config::{Layout, ReadyLayout};
use serenity::all::{
    ChannelId, ChannelType, Context, CreateChannel, GuildId, Http, UserId, VoiceState,
};
use session::{Lease, Session};
use state::RoomBook;
use std::{collections::HashSet, time::Instant};
use store::Store;
use tokio::sync::Mutex;

/// Construction guarantees Discord's length/control-character constraints.
struct RoomName(String);
impl RoomName {
    fn for_member(display: &str) -> Self {
        let clean: String = display
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        let clean = clean.trim();
        Self(format!(
            "{}'s room",
            if clean.is_empty() { "Player" } else { clean }
        ))
    }
}

struct Runtime {
    book: RoomBook,
    revision: Option<u64>,
}
pub struct Rooms {
    config: Layout,
    store: Store,
    runtime: Mutex<Runtime>,
    session: Session,
}
fn missing(error: &serenity::Error) -> bool {
    matches!(error, serenity::Error::Http(e) if e.status_code().is_some_and(|s| s.as_u16() == 404))
}
impl Rooms {
    pub fn open(config: Config) -> Result<Self, &'static str> {
        let (store, book) = Store::open(config.state_path, &config.layout)?;
        Ok(Self {
            config: config.layout,
            store,
            runtime: Mutex::new(Runtime {
                book,
                revision: None,
            }),
            session: Session::default(),
        })
    }
    pub fn disconnect(&self) {
        self.session.disconnect();
    }
    pub fn guild_removed(&self, guild: GuildId) {
        if guild == self.config.guild() {
            self.disconnect();
        }
    }
    pub async fn synchronize(&self, ctx: &Context, guild: GuildId) {
        if guild != self.config.guild() {
            return;
        }
        let attempt = self.session.begin();
        let bot = ctx.cache.current_user().id;
        match ReadyLayout::resolve(&self.config, &ctx.http, bot).await {
            Ok(layout) => {
                self.session.finish(attempt, layout);
                eprintln!("Voice room layout resolved");
            }
            Err(reason) => eprintln!("Voice rooms unavailable: {reason}"),
        }
    }
    pub async fn resume(&self, ctx: &Context) {
        self.synchronize(ctx, self.config.guild()).await;
    }
    fn in_lobby(&self, ctx: &Context, user: UserId) -> bool {
        ctx.cache.guild(self.config.guild()).is_some_and(|g| {
            g.voice_states
                .get(&user)
                .is_some_and(|v| v.channel_id == Some(self.config.lobby()))
        })
    }
    fn occupied(&self, ctx: &Context) -> Option<HashSet<ChannelId>> {
        ctx.cache.guild(self.config.guild()).map(|g| {
            g.voice_states
                .values()
                .filter_map(|v| v.channel_id)
                .collect()
        })
    }
    fn reset_session(runtime: &mut Runtime, lease: &Lease) {
        if runtime.revision != Some(lease.revision()) {
            runtime.book.reset_grace(Instant::now());
            runtime.revision = Some(lease.revision());
        }
    }
    pub async fn voice_changed(&self, ctx: &Context, old: Option<VoiceState>, new: VoiceState) {
        if new.guild_id != Some(self.config.guild())
            || old.and_then(|v| v.channel_id) == new.channel_id
        {
            return;
        }
        let Some(lease) = self.session.lease() else {
            return;
        };
        // A single operation owns the registry through creation and movement. Cleanup cannot
        // interleave, so a separate in-flight map (and impossible partial transfers) isn't needed.
        let mut runtime = self.runtime.lock().await;
        if !self.session.current(&lease) {
            return;
        }
        Self::reset_session(&mut runtime, &lease);
        let Some(occupied) = self.occupied(ctx) else {
            return;
        };
        runtime.book.observe(&occupied, Instant::now());
        if new.channel_id != Some(self.config.lobby()) || !self.in_lobby(ctx, new.user_id) {
            return;
        }
        let Some(member) = new.member else {
            eprintln!("Ignoring voice arrival without member data");
            return;
        };
        if member.user.bot {
            return;
        }
        let name = RoomName::for_member(member.display_name());
        let result = async {
            let channel = self
                .ensure_room(&ctx.http, &mut runtime.book, &lease, new.user_id, name)
                .await?;
            // Recheck live state after HTTP; never pull someone back after they leave the lobby.
            if self.session.current(&lease) && self.in_lobby(ctx, new.user_id) {
                self.config
                    .guild()
                    .move_member(&ctx.http, new.user_id, channel)
                    .await
                    .map_err(
                        |_| "Could not move member; check Move Members and Connect permissions",
                    )?;
                eprintln!("Moved member into a temporary voice room");
            }
            Ok::<_, &'static str>(())
        }
        .await;
        if let Err(reason) = result {
            eprintln!("Voice room request failed: {reason}");
        }
        // If a transfer failed, the unoccupied room is safely eligible for cleanup.
    }
    async fn ensure_room(
        &self,
        http: &Http,
        book: &mut RoomBook,
        lease: &Lease,
        owner: UserId,
        name: RoomName,
    ) -> Result<ChannelId, &'static str> {
        let layout = lease.layout().layout();
        if let Some(id) = book.existing(owner) {
            match id.to_channel(http).await {
                Ok(c) => {
                    if c.guild().is_some_and(|c| layout.is_room(&c)) {
                        return Ok(id);
                    }
                    book.forget(id);
                }
                Err(e) if missing(&e) => book.forget(id),
                Err(_) => return Err("Cannot check existing voice room"),
            }
        }
        if book.is_full() {
            return Err("All 20 temporary rooms are in use");
        }
        self.store.save(book)?;
        if !self.session.current(lease) {
            return Err("Discord session changed; try joining again");
        }
        let channel = layout
            .guild()
            .create_channel(
                http,
                CreateChannel::new(name.0)
                    .kind(ChannelType::Voice)
                    .category(layout.category())
                    .audit_log_reason("Member joined the room creation channel"),
            )
            .await
            .map_err(|_| "Cannot create voice room")?;
        book.insert(owner, channel.id, Instant::now())?;
        // Keep the ID in memory on a disk error, so cleanup can still remove this room.
        self.store.save(book)?;
        Ok(channel.id)
    }
    pub async fn tick(&self, ctx: &Context) {
        let Some(lease) = self.session.lease() else {
            return;
        };
        let mut runtime = self.runtime.lock().await;
        if !self.session.current(&lease) {
            return;
        }
        Self::reset_session(&mut runtime, &lease);
        let Some(occupied) = self.occupied(ctx) else {
            return;
        };
        let expired = runtime.book.observe(&occupied, Instant::now());
        if expired.is_empty() {
            return;
        }
        let Ok(channels) = self.config.guild().channels(&ctx.http).await else {
            return;
        };
        for id in expired {
            match channels.get(&id) {
                None => {
                    runtime.book.forget(id);
                    continue;
                }
                Some(c) if !self.config.is_room(c) => {
                    runtime.book.forget(id);
                    continue;
                }
                Some(_) => {}
            }
            let Some(occupied) = self.occupied(ctx) else {
                break;
            };
            if !self.session.current(&lease) {
                break;
            }
            if occupied.contains(&id) {
                runtime.book.observe(&occupied, Instant::now());
                continue;
            }
            match id.delete(&ctx.http).await {
                Ok(_) => {
                    runtime.book.forget(id);
                    eprintln!("Removed empty temporary voice room");
                }
                Err(e) if missing(&e) => runtime.book.forget(id),
                Err(_) => eprintln!("Could not delete an empty voice room"),
            }
        }
        if let Err(reason) = self.store.save(&runtime.book) {
            eprintln!("{reason}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_arrivals_reuse_the_persisted_room() {
        use serenity::all::GuildChannel;
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let layout: Layout = serde_json::from_str(r#"{"guild":1,"category":2,"lobby":3}"#).unwrap();
        let mut channel = GuildChannel::default();
        channel.id = ChannelId::new(4);
        channel.guild_id = GuildId::new(1);
        channel.parent_id = Some(ChannelId::new(2));
        channel.kind = ChannelType::Voice;
        channel.name = "Player's room".into();
        let body = serde_json::to_string(&channel).unwrap();
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                methods.push(header.lines().next().unwrap().to_owned());
                let len = header
                    .lines()
                    .find_map(|s| {
                        s.to_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let mut payload = vec![0; len];
                stream.read_exact(&mut payload).unwrap();
                if header.starts_with("POST") {
                    let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                    assert_eq!(payload["name"], "Player's room");
                    assert_eq!(payload["parent_id"], "2");
                    assert_eq!(payload["type"], 2);
                }
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
            methods
        });
        let path = std::env::temp_dir().join(format!(
            "discord-room-test-{}-{}.json",
            std::process::id(),
            addr.port()
        ));
        let rooms = Rooms::open(Config {
            layout: layout.clone(),
            state_path: path.clone(),
        })
        .unwrap();
        let resolution = rooms.session.begin();
        rooms
            .session
            .finish(resolution, ReadyLayout::fixture(layout.clone()));
        let lease = rooms.session.lease().unwrap();
        let http = serenity::http::HttpBuilder::new("test-token")
            .proxy(format!("http://{addr}"))
            .ratelimiter_disabled(true)
            .build();
        let request = || async {
            let mut runtime = rooms.runtime.lock().await;
            rooms
                .ensure_room(
                    &http,
                    &mut runtime.book,
                    &lease,
                    UserId::new(5),
                    RoomName::for_member("Player"),
                )
                .await
        };
        let (a, b) = tokio::join!(request(), request());
        assert_eq!(a, Ok(ChannelId::new(4)));
        assert_eq!(b, a);
        let methods = server.join().unwrap();
        assert!(methods[0].starts_with("POST "));
        assert!(methods[1].starts_with("GET "));
        let saved =
            RoomBook::parse(&std::fs::read(&path).unwrap(), &layout, Instant::now()).unwrap();
        assert_eq!(saved.existing(UserId::new(5)), Some(ChannelId::new(4)));
        rooms.disconnect();
        let mut runtime = rooms.runtime.lock().await;
        assert!(
            rooms
                .ensure_room(
                    &http,
                    &mut runtime.book,
                    &lease,
                    UserId::new(6),
                    RoomName::for_member("Other")
                )
                .await
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn generated_names_are_bounded_and_handle_empty_unicode_names() {
        assert_eq!(RoomName::for_member("  ").0, "Player's room");
        assert_eq!(RoomName::for_member("Загін 🐺\n").0, "Загін 🐺's room");
        let name = RoomName::for_member(&"я".repeat(150));
        assert!(name.0.chars().count() < 100);
        assert!(!name.0.chars().any(char::is_control));
    }
}
