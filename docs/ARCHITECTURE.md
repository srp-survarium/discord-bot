# Architecture

The binary owns startup, shutdown, background tasks, and event routing. Features
own behavior. There is no plugin framework: adding a feature means adding its
module, parsing its configuration, and forwarding the events it needs.

| Module | Responsibility |
| --- | --- |
| `config` | Environment boundary and a nonempty secret token type |
| `rooms/config` | Complete layout parsing and resolving Discord channel types, hierarchy, and permissions |
| `rooms/session` | Offline, resolving, and ready states; revisions reject stale asynchronous work |
| `rooms/state` | Ownership registry and occupancy transitions, independent of Discord I/O |
| `rooms/store` | Versioned state parsing and atomic file replacement |
| `rooms` | Voice arrivals, creation, movement, and cleanup orchestration |
| `counters/config` | Optional polling and distinct channel targets |
| `counters/source` | Endpoint/poll interval types and JSON/HTML adapters |
| `counters/channels` | Renaming and rate-limit cooldown state |
| `counters` | Polling, source snapshots, and presence updates |

## Parse at boundaries

Untrusted environment values and persisted JSON are parsed before features start.
Domain types have private fields and expose constructors that preserve invariants:

- `BotToken` is nonempty and deliberately has no Debug or Display implementation.
- `Endpoint` contains an HTTP(S) URL allowed by the transport policy. `Source`
  accepts this type, so callers cannot bypass policy by passing any URL.
- `PollInterval` is a nonzero duration within the configured bounds.
- `Layout` requires distinct, nonzero Discord IDs. `ReadyLayout` additionally
  requires a successful Discord lookup of channel types, hierarchy, and permissions.
- `RoomName` is constructed from a display name with bounded length and no controls.
- `RoomBook` rejects duplicate owners/channels, reserved IDs, and excess capacity.
  Mutation stays within its module. Persistence parses back through these rules.
- Counter snapshots distinguish `Pending`, `Unavailable`, and `Available`.
  A valid zero is always available data.

## Room transitions and concurrency

Each owned room is either occupied or empty since a particular instant. Timers
cannot exist without a room. A single mutex protects the registry through each
create-and-move operation, preventing cleanup or duplicate arrivals from
interleaving. Gateway cache updates continue while HTTP operations are pending.
Before moving or deleting, the bot rechecks the current cache and session revision.

A ready session carries a resolved layout. Disconnecting invalidates its revision;
a lookup finishing later cannot restore readiness. Reconnection reparses Discord
state and gives empty rooms a fresh grace period. The bot never interprets a
missing guild cache as proof that every room is empty.

## Limits that types cannot remove

Discord permissions and occupancy can change after a lookup. HTTP errors remain
possible, and Discord has no atomic “delete only if still empty” operation. There
is a narrow race between the final occupancy check and channel deletion. Likewise,
a member can leave between the final movement check and the move request.

Creating a channel and recording it on disk are separate operations. A process
crash or disk failure between them can leave an untracked room. The bot retains
that ID in memory on a write failure and fails closed on the next state write,
but it never guesses ownership from channel names after a restart. Such an orphan
needs manual cleanup. Atomic file replacement avoids partially written JSON;
it is not a transaction with Discord.

The room-operation mutex favors simple ownership rules over parallel throughput.
A Discord rate-limit wait can delay other room operations. Counter requests run
in separate tasks. Use one process; the state store is not a multi-process database.
