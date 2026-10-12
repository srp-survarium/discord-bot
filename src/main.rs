// SPDX-License-Identifier: GPL-3.0-or-later
mod config;
mod counters;
mod moderation;
mod rooms;

use config::{BotToken, Environment};
use serenity::{
    Client,
    all::{
        ConnectionStage, Context, EventHandler, GatewayIntents, Guild, Ready, ResumedEvent,
        ShardStageUpdateEvent, UnavailableGuild, VoiceState,
    },
    async_trait,
};
use std::{env, process::ExitCode, sync::Arc, time::Duration};
use tokio::sync::watch;

/// Discord event routing only. Each feature owns its domain logic and state.
struct Handler {
    gateway: watch::Sender<Option<Context>>,
    rooms: Option<Arc<rooms::Rooms>>,
}
#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, _: Ready) {
        eprintln!("Discord connected");
        self.gateway.send_replace(Some(ctx));
    }
    async fn resume(&self, ctx: Context, _: ResumedEvent) {
        self.gateway.send_replace(Some(ctx.clone()));
        if let Some(rooms) = &self.rooms {
            rooms.resume(&ctx).await;
        }
    }
    async fn guild_create(&self, ctx: Context, guild: Guild, _: Option<bool>) {
        if let Some(rooms) = &self.rooms {
            rooms.synchronize(&ctx, guild.id).await;
        }
    }
    async fn guild_delete(&self, _: Context, guild: UnavailableGuild, _: Option<Guild>) {
        if let Some(rooms) = &self.rooms {
            rooms.guild_removed(guild.id);
        }
    }
    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        if let Some(rooms) = &self.rooms {
            rooms.voice_changed(&ctx, old, new).await;
        }
    }
    async fn shard_stage_update(&self, _: Context, event: ShardStageUpdateEvent) {
        if event.new != ConnectionStage::Connected {
            self.gateway.send_replace(None);
            if let Some(rooms) = &self.rooms {
                rooms.disconnect();
            }
        }
    }
}

enum Command {
    Run,
    CheckSource,
    CheckGuard,
}
impl Command {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, &'static str> {
        match args.collect::<Vec<_>>().as_slice() {
            [] => Ok(Self::Run),
            [arg] if arg == "--check-source" => Ok(Self::CheckSource),
            [arg] if arg == "--check-guard" => Ok(Self::CheckGuard),
            _ => Err("Usage: discord-bot [--check-source | --check-guard]"),
        }
    }
}

async fn run() -> Result<(), String> {
    let command = Command::parse(env::args().skip(1))?;
    let env = Environment::process();
    let counters = counters::Config::parse(&env)?;
    if matches!(command, Command::CheckSource) {
        let config = counters.ok_or("Set SRP_ONLINE_URL to check a counter source")?;
        let stats = config.source.fetch().await?;
        println!("Players online: {}", stats.online);
        if let Some(registered) = stats.registered {
            println!("Registered: {registered}");
        }
        if let Some(matches) = stats.matches {
            println!("Matches: {matches}");
        }
        if let Some(queued) = stats.queued {
            println!("Queued: {queued}");
        }
        return Ok(());
    }
    let token: BotToken = env.required("DISCORD_BOT_TOKEN")?;
    let guard = moderation::Config::parse(&env)?;
    if matches!(command, Command::CheckGuard) {
        let guard = guard.ok_or("Configure DISCORD_GUARD_GUILD_ID and DISCORD_GUARD_ROLE_ID")?;
        moderation::check(&guard, &serenity::all::Http::new(token.expose())).await?;
        println!(
            "Moderation guard ready: 16 people per rolling 24 hours, 11 per hour or 3 per 30 seconds; manual restoration only"
        );
        return Ok(());
    }
    let room_config = rooms::Config::parse(&env)?;
    if let (Some(counters), Some(rooms)) = (&counters, &room_config)
        && counters.targets.ids().any(|id| rooms.layout.reserved(id))
    {
        return Err("Counter channels cannot be the voice entry channel or category".into());
    }
    let rooms = room_config
        .map(rooms::Rooms::open)
        .transpose()?
        .map(Arc::new);
    let intents = if rooms.is_some() {
        GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES
    } else {
        GatewayIntents::empty()
    };
    let (sender, receiver) = watch::channel(None);
    let mut client = Client::builder(token.expose(), intents)
        .event_handler(Handler {
            gateway: sender,
            rooms: rooms.clone(),
        })
        .await
        .map_err(|_| "Could not initialize Discord; check the token and network access")?;
    let mut tasks = Vec::new();
    if let Some(guard) = guard {
        tasks.push(moderation::start(guard, client.http.clone()));
    }
    if let Some(counters) = counters {
        tasks.extend(counters::start(
            counters,
            client.http.clone(),
            receiver.clone(),
        ));
    }
    if let Some(rooms) = rooms {
        tasks.push(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let ctx = receiver.borrow().clone();
                if let Some(ctx) = ctx {
                    rooms.tick(&ctx).await;
                }
            }
        }));
    }
    let result = tokio::select! {
        result = client.start() => result.map_err(|_| "Discord gateway failed"),
        result = shutdown() => result.map_err(|_| "Could not listen for shutdown"),
    };
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
    client.shard_manager.shutdown_all().await;
    result.map_err(Into::into)
}
async fn shutdown() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}
