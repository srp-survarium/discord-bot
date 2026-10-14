<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Discord bot

A Rust Discord bot with independent features for temporary voice rooms and
optional server-stat counters. Extracted from
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
is needed. Ctrl+C or SIGTERM stops it. For background operation, see
[Run on a server](#run-on-a-server), or point your service supervisor at
`target/release/discord-bot`, set the working directory to this repo, and load
`.env`. Run one instance per set of Discord channels and state file.

On NixOS, if `cc` is missing:

```sh
nix-shell -p gcc --run 'bash run.sh'
```

## Run on a server

`deploy/deploy.sh` runs the bot as a systemd service on any x86-64 Linux host
you can reach as root over SSH. It needs Nix locally, not on the server. It
builds the committed `HEAD` as a static binary (`deploy/static.nix`), so a
NixOS-built `target/release` binary is never copied to another distribution.

```sh
deploy/deploy.sh --setup root@your-server  # First time: user, directories, unit, .env.
deploy/deploy.sh root@your-server          # Later: rebuild, replace, restart.
```

`--setup` creates a `discord-bot` system user, installs
[`deploy/discord-bot.service`](deploy/discord-bot.service) and copies `.env` to
`/etc/discord-bot/env` (root-only). Repeat it after changing `.env` or the unit.
The binary goes to `/opt/discord-bot` with the deployed commit in `revision`, and
the working directory, holding `.rooms.json`, is `/var/lib/discord-bot`. When
moving the bot, stop the old instance first and copy its `.rooms.json` there,
owned by `discord-bot`. Uncommitted changes to tracked files stop the deploy.

```sh
ssh root@your-server journalctl -u discord-bot -f  # Follow the logs.
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

Set `SRP_ONLINE_URL` to enable polling. For Skhid, use the public server API:

```dotenv
SRP_ONLINE_URL='https://026g.skhid.link/api/stats'
SRP_SOURCE_FORMAT='json'
SRP_POLL_SECONDS=10
```

The endpoint returns `{"online":14,"registered":72,"matches":2,"queued":1}`.
It needs no account token. The bot's activity shows `14 online · 2 matches · 1 queued`.
It refreshes from each poll when counts change (every 10 seconds by default),
independently of the channel rename cooldown. Registered accounts stay in the
optional channel counter. If the source fails, the activity shows
`Server stats unavailable`; missing optional fields are omitted from the activity.
Each sidebar counter is optional; set its ID to a different voice channel:

| Setting | Channel name |
| --- | --- |
| `DISCORD_ONLINE_CHANNEL_ID` | 🟢 Online: 14 |
| `DISCORD_REGISTERED_CHANNEL_ID` | 👥 Registered: 72 |
| `DISCORD_MATCHES_CHANNEL_ID` | 🎮 Matches: 2 |
| `DISCORD_QUEUED_CHANNEL_ID` | ⏳ Queued: 1 |

Give the bot **View Channel**,
**Manage Channels**, and **Connect** on those channels. To make them labels, deny
**Connect** for everyone and explicitly allow it for the bot.

`SRP_POLL_SECONDS` defaults to 10 and accepts 10–86400. Channel renames are limited
to once every five minutes per channel, only when changed. Failed polls show
unavailable rather than zero. Remote sources must use HTTPS; local HTTP is allowed.
Redirects are rejected and requests time out after five seconds. Set
`SRP_ACCOUNT_TOKEN` only for a JSON source requiring a bearer credential.

```sh
bash run.sh --check-source  # Fetch counts without connecting to Discord.
```

Other JSON sources must include `online`; `registered`, `matches`, and `queued`
are optional. Counts must be nonnegative integers. Missing optional counts show
unavailable, not zero. The legacy `skhid` HTML adapter remains available for older
servers, but supplies only online and registered counts. Use JSON for the new counters.
When the bot is offline, channel names retain their last values and room creation
and cleanup stop.

## Optional moderation guard

Select the server and the moderation role the bot may remove:

```dotenv
DISCORD_GUARD_GUILD_ID='your-server-id'
DISCORD_GUARD_ROLE_ID='moderation-role-id'
```

Give the bot **Manage Roles** and **View Audit Log**, and place its highest role
above the guarded role. The guarded role must grant **Ban Members** or **Kick
Members** and must not be an integration-managed role or `@everyone`. Leave both
settings blank to disable this feature. Voice rooms and counters remain independent.

The guard checks audit logs about every five seconds. It removes the configured
role from the acting moderator on any threshold:

- The **11th distinct person** kicked or banned within the preceding 60 minutes.
- The **16th distinct person** kicked or banned within the preceding 24 hours.
- The **3rd distinct person** kicked or banned within the preceding 30 seconds.

Kicks and bans share each limit. Repeated actions against the same person count
once per window. Ordinary departures, unbans and message events do not count.
The daily limit is a rolling 24-hour window, with no reset at midnight.
The server owner and the bot itself are excluded. Only the selected role is
removed; unrelated roles remain. Role removal has an audit-log reason. Another
role can still grant moderation powers, so avoid overlapping grants for protected
moderators; the guard reports this case rather than removing unrelated roles.

There is **no automatic restoration**, even after the window expires or the bot
restarts. An owner/admin restores the role manually after investigating. A role
restored while its holder still exceeds a limit will be removed again. Audit
history supplies the rolling counts across restarts; no counter state file is
needed. Failed removals are reported and retried after a one-minute backoff.

```sh
bash run.sh --check-guard  # Read-only permission, hierarchy and audit-access check.
```

This is reactive protection: actions already completed are not undone, and more
actions can occur before Discord supplies the audit entries and the bot removes
the role. It cannot protect the owner, run while the bot is offline, or overcome
missing permissions. Scans have a 60-second timeout and a 100-page limit per action
type; incomplete scans are reported and retried, and any breaches already found
are acted on before a later page can fail. Permission/hierarchy failures are
reported without stopping the other features.

To restrict deletion of other people's messages, remove **Manage Messages** from
moderator roles and channel/member overrides, and avoid **Administrator** grants.
Native **Ban Members** access can still delete the banned person's recent message
history. Preventing that requires routing bans through controlled bot commands
that explicitly preserve messages; this guard does not provide those commands.

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
