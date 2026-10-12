// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::config::Environment;
use serde_json::{Value, json};
use serenity::{
    all::{RoleId, User},
    http::HttpBuilder,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[derive(Default)]
struct State {
    removed: bool,
    reject_removal: bool,
    paginated: bool,
    daily: bool,
    owner: bool,
    requests: Vec<(String, String)>,
}
struct Mock {
    http: Http,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn config() -> Config {
    Config::parse(&Environment::new(|key| match key {
        "DISCORD_GUARD_GUILD_ID" => Some("1".into()),
        "DISCORD_GUARD_ROLE_ID" => Some("2".into()),
        _ => None,
    }))
    .unwrap()
    .unwrap()
}
fn user(id: u64) -> User {
    let mut user = User::default();
    user.id = UserId::new(id);
    user.name = "fixture".into();
    user
}
fn member(id: u64, roles: &[u64]) -> Value {
    let mut member = Member::default();
    member.user = user(id);
    member.roles = roles.iter().copied().map(RoleId::new).collect();
    serde_json::to_value(member).unwrap()
}
fn guild() -> Value {
    let role = |id, position, permissions| {
        let mut role = Role::default();
        role.id = RoleId::new(id);
        role.position = position;
        role.permissions = permissions;
        role
    };
    json!({
        "id": "1", "name": "fixture", "owner_id": "99", "verification_level": 0,
        "default_message_notifications": 0, "explicit_content_filter": 0,
        "roles": [role(1, 0, Permissions::empty()), role(2, 1, Permissions::BAN_MEMBERS),
            role(3, 3, Permissions::MANAGE_ROLES | Permissions::VIEW_AUDIT_LOG),
            role(4, 2, Permissions::empty())],
        "emojis": [], "features": [], "mfa_level": 0, "system_channel_flags": 0,
        "premium_tier": 0, "preferred_locale": "en-US", "nsfw_level": 0,
        "stickers": [], "premium_progress_bar_enabled": false
    })
}
fn audit(entries: Vec<Value>) -> Value {
    json!({"audit_log_entries": entries, "auto_moderation_rules": [],
        "application_commands": [], "guild_scheduled_events": [], "integrations": [],
        "threads": [], "users": [], "webhooks": []})
}
impl Mock {
    fn new(initial: State) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let http = HttpBuilder::new("fixture-token")
            .proxy(format!("http://{}", listener.local_addr().unwrap()))
            .ratelimiter_disabled(true)
            .build();
        let state = Arc::new(Mutex::new(initial));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let shutdown = stop.clone();
        let thread = thread::spawn(move || {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let snowflake = (now - 5_000 - 1_420_070_400_000) << 22;
            while !shutdown.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                let mut words = request.split_whitespace();
                let method = words.next().unwrap();
                let path = words.next().unwrap();
                let mut state = shared.lock().unwrap();
                state.requests.push((method.into(), path.into()));
                let mut status = "200 OK";
                let body = if path.ends_with("/users/@me") {
                    serde_json::to_value(user(42)).unwrap()
                } else if path.ends_with("/guilds/1") {
                    guild()
                } else if path.ends_with("/members/42") {
                    member(42, &[3])
                } else if path.ends_with("/members/7") {
                    member(7, if state.removed { &[4] } else { &[2, 4] })
                } else if path.contains("/audit-logs?") {
                    let snowflake = if state.daily {
                        // Both pages are outside the hourly window; the second is older.
                        let hours = if path.contains("before=") { 3 } else { 2 };
                        (now - hours * 3_600_000 - 1_420_070_400_000) << 22
                    } else {
                        snowflake
                    };
                    let entry = |index: u64, target: u64| {
                        json!({
                            "id": (snowflake + index).to_string(),
                            "user_id": if state.owner { "99" } else { "7" },
                            "target_id": target.to_string(), "action_type": 22
                        })
                    };
                    if path.contains("action_type=20") {
                        audit(vec![])
                    } else if state.paginated && !path.contains("before=") {
                        audit((100..200).rev().map(|i| entry(i, 100)).collect())
                    } else if state.daily {
                        audit((1..=15).rev().map(|i| entry(i, 100 + i)).collect())
                    } else {
                        audit(vec![entry(3, 100), entry(2, 101), entry(1, 102)])
                    }
                } else if method == "DELETE" && path.ends_with("/members/7/roles/2") {
                    if state.reject_removal {
                        status = "403 Forbidden";
                        json!({"code": 50013, "message": "fixture denial"})
                    } else {
                        state.removed = true;
                        status = "204 No Content";
                        Value::Null
                    }
                } else {
                    panic!("unexpected fixture request: {method} {path}");
                };
                let body = if status.starts_with("204") {
                    String::new()
                } else {
                    body.to_string()
                };
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            http,
            state,
            stop,
            thread: Some(thread),
        }
    }
}

#[tokio::test]
async fn removes_only_guarded_role_and_backfill_does_not_repeat_mutation() {
    let mock = Mock::new(State::default());
    scan(&config(), &mock.http).await.unwrap();
    scan(&config(), &mock.http).await.unwrap();
    let state = mock.state.lock().unwrap();
    assert!(state.removed);
    let deletes: Vec<_> = state
        .requests
        .iter()
        .filter(|(method, _)| method == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 1);
    assert!(deletes[0].1.ends_with("/members/7/roles/2"));
}

#[tokio::test]
async fn failed_removal_is_reported_and_retried_on_next_scan() {
    let mock = Mock::new(State {
        reject_removal: true,
        ..State::default()
    });
    assert!(scan(&config(), &mock.http).await.is_err());
    assert!(!mock.state.lock().unwrap().removed);
    mock.state.lock().unwrap().reject_removal = false;
    scan(&config(), &mock.http).await.unwrap();
    assert!(mock.state.lock().unwrap().removed);
}

#[tokio::test]
async fn follows_audit_pages_and_deduplicates_people() {
    let mock = Mock::new(State {
        paginated: true,
        ..State::default()
    });
    scan(&config(), &mock.http).await.unwrap();
    let state = mock.state.lock().unwrap();
    assert!(state.removed);
    assert!(
        state
            .requests
            .iter()
            .any(|(_, path)| path.contains("before="))
    );
}

#[tokio::test]
async fn daily_limit_reads_older_pages_beyond_the_hourly_window() {
    let mock = Mock::new(State {
        paginated: true,
        daily: true,
        ..State::default()
    });
    scan(&config(), &mock.http).await.unwrap();
    let state = mock.state.lock().unwrap();
    assert!(state.removed);
    assert!(
        state
            .requests
            .iter()
            .any(|(_, path)| path.contains("before="))
    );
}

#[tokio::test]
async fn owner_cannot_be_suspended() {
    let mock = Mock::new(State {
        owner: true,
        ..State::default()
    });
    scan(&config(), &mock.http).await.unwrap();
    assert!(!mock.state.lock().unwrap().removed);
}
