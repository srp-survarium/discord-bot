# Development

Keep this bot generic and modular. Feature behavior belongs in its feature module;
main handles configuration, lifecycle, and event routing. New features should be
independently configurable without coupling them to Skhid or existing features.

Use parse-don't-validate design. Parse environment, disk, and API inputs into domain
types at boundaries. Prefer private fields, typed IDs, and enum states over raw
strings, flags, nested options, or parallel collections. Constructors and mutation
methods should preserve invariants. Avoid adding a framework until a concrete
feature requires it. Remote permissions and occupancy remain fallible even after
parsing; handle errors and recheck where races matter.

Preserve join-to-create rooms, their persisted ownership, and optional counters.
Do not publish .env, room-state files, credentials, live identifiers, or logs.
Run cargo fmt --check, cargo clippy --locked --all-targets -- -D warnings, and
cargo test --locked for behavioral changes. Tests should exercise invariants and
failure paths, with fixtures and local mock services rather than live Discord.
