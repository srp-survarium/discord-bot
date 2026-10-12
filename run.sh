#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
bot_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$bot_dir"
if [[ -f .env ]]; then
    set -a
    source .env
    set +a
fi
exec cargo run --locked --release -- "$@"
