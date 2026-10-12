<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Discord bot

A Rust Discord bot with independent features for temporary voice rooms and
optional website counters. Extracted from
[srp-survarium/srp](https://github.com/srp-survarium/srp). GPL-3.0-or-later.

## Run locally

Install a current Rust toolchain with Cargo and a C linker. Then:

```sh
cp .env.example .env
chmod 600 .env
# Fill in the bot token and settings for the features you want.
bash run.sh
```

Create and invite your bot through the
[Discord Developer Portal](https://discord.com/developers/applications).
No privileged gateway intents are required. Keep the token in `.env`; never
commit it. Leave a feature's settings blank to disable it.

The bot runs while your PC is awake and connected. No incoming port or web server
is needed. Ctrl+C or SIGTERM stops it. For background operation, point your service
supervisor at `target/release/discord-bot`, set the working directory to this repo,
and load `.env`. Run one instance per set of Discord channels and state file.

On NixOS, if `cc` is missing:

```sh
nix-shell -p gcc --run 'bash run.sh'
```

## Join-to-create voice rooms

Create a **Click to create** voice channel inside a voice category, such as
**Voice Channels**. Set all three values:

```dotenv
DISCORD_VOICE_GUILD_ID='your-server-id'
DISCORD_VOICE_CATEGORY_ID='voice-category-id'
DISCORD_VOICE_LOBBY_CHANNEL_ID='entry-voice-channel-id'
```

Joining creates a public room named after the member and moves them into it.
Joining again returns them to their existing room. Rooms are created in the same
category, inherit its permissions, and are removed roughly 5–10 seconds after
everyone leaves. There is one active room per member, with a maximum of 20.

Give the bot **View Channel**, **Connect**, **Manage Channels**, and **Move Members**
on both the category and entry channel. Members need **View Channel**, **Connect**,
and **Speak** in the category. The bot does not join voice itself.

Room ownership is stored in `.rooms.json`. Preserve it across restarts and keep
it out of Git. `DISCORD_VOICE_STATE_PATH` can select another location. Invalid,
duplicate, or mismatched saved state stops startup. Only recorded room IDs within
the configured category are eligible for cleanup. Moving a room out of the category
releases it from management. Existing channels and the entry channel are preserved.

## Optional counters

Set `SRP_ONLINE_URL` to enable polling. JSON sources return `{"online":12}` with
an optional `registered` integer. For the Skhid English status page:

```dotenv
SRP_ONLINE_URL='https://026g.skhid.link/?lang=en'
SRP_SOURCE_FORMAT='skhid'
```

The bot displays the count in its activity. Optionally set
`DISCORD_ONLINE_CHANNEL_ID` and `DISCORD_REGISTERED_CHANNEL_ID` to two different
voice channels to show sidebar counters. Give the bot **View Channel**,
**Manage Channels**, and **Connect** on those channels. To make them labels, deny
**Connect** for everyone and explicitly allow it for the bot.

`SRP_POLL_SECONDS` defaults to 60 and accepts 15–86400. Channel renames are limited
to once every five minutes per channel, only when changed. Failed polls show
unavailable rather than zero. Remote sources must use HTTPS; local HTTP is allowed.
Redirects are rejected and requests time out after five seconds. Set
`SRP_ACCOUNT_TOKEN` only for a JSON source requiring a bearer credential.

```sh
bash run.sh --check-source  # Fetch counts without connecting to Discord.
```

The HTML adapter parses labelled stats and may need updating if the site changes.
When the bot is offline, channel names retain their last values and room creation
and cleanup stop.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

Tests use fixtures and local mock HTTP servers; no Discord token is needed.
See [architecture](docs/ARCHITECTURE.md) for feature boundaries, invariants, and
remaining limits, and [review notes](docs/REVIEW.md) for the extraction review.
