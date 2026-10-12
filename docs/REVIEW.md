# Extraction review

Scope: temporary voice rooms and optional counters extracted into a standalone
Rust package. No game-server code or live credentials are part of this repository.

## Findings addressed

- URL policy returned a general URL, allowing callers to bypass checks. `Endpoint`
  now carries that policy and is the only accepted input to the source constructor.
- Nested options obscured pending, failed, and successful polling states. A named
  snapshot enum expresses those cases, with failure and zero remaining distinct.
- Raw IDs and independent ownership, timer, and transfer collections could disagree.
  Typed Discord IDs and an encapsulated registry enforce ownership constraints.
  Occupancy is stored with each room, and transfers hold the registry lock.
- Channel-layout checking only logged failure while a separate boolean could mark
  rooms available. Ready sessions now require a resolved layout; session revisions
  also reject stale lookup completions after disconnects.
- Generated room names were immediately revalidated as strings. Construction now
  returns a `RoomName` that the creation path accepts directly.
- Configuration and counter logic were tied to the original game workspace.
  Features are independently optional, with environment parsing at startup.
- Shutdown only handled Ctrl+C. The standalone binary also handles SIGTERM.

## Verification

The test suite covers rejected configurations and persisted ownership, cooldowns,
HTTP errors, redirects and response bounds, HTML/JSON parsing, occupancy grace
periods, restart recovery, and stale session resolution. Formatting and Clippy
are enforced in CI along with tests.

Remaining distributed-system limits and operational assumptions are documented in
[architecture](ARCHITECTURE.md). The type design removes invalid local states;
it does not claim remote Discord state remains constant after a lookup.
