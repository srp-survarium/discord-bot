#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the committed HEAD as a static binary and run it on a systemd host.
#   deploy/deploy.sh [--setup] <ssh-host>
# The SSH login must be root. --setup also creates the discord-bot user and
# directories, installs the unit and copies .env; it is safe to repeat.
set -euo pipefail

usage() {
    echo "usage: deploy/deploy.sh [--setup] <ssh-host>" >&2
    exit 2
}

setup=0
if [[ ${1-} == --setup ]]; then
    setup=1
    shift
fi
[[ $# -eq 1 && $1 != -* ]] || usage
host=$1

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

if ! git diff --quiet HEAD --; then
    echo "Tracked files have uncommitted changes; commit them first (the build uses HEAD)." >&2
    exit 1
fi
rev="$(git rev-parse HEAD)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "Building $rev"
nix build --impure --file deploy/static.nix --argstr rev "$rev" --out-link "$work/result"

if ((setup)); then
    [[ -f .env ]] || { echo "No .env to install; create it first (see .env.example)." >&2; exit 1; }
    echo "Setting up $host"
    ssh "$host" 'set -e
        id discord-bot >/dev/null 2>&1 ||
            useradd --system --home-dir /var/lib/discord-bot --shell /usr/sbin/nologin discord-bot
        install -d -m 755 /opt/discord-bot
        install -d -m 700 -o discord-bot -g discord-bot /var/lib/discord-bot
        install -d -m 700 /etc/discord-bot'
    scp -q deploy/discord-bot.service "$host:/etc/systemd/system/discord-bot.service"
    scp -q .env "$host:/etc/discord-bot/env"
    ssh "$host" 'set -e
        chown root:root /etc/discord-bot/env
        chmod 600 /etc/discord-bot/env
        systemctl daemon-reload
        systemctl enable --quiet discord-bot.service'
fi

# A running executable cannot be overwritten, so upload beside it and rename.
echo "Installing on $host"
scp -q "$work/result/bin/discord-bot" "$host:/opt/discord-bot/discord-bot.new"
# shellcheck disable=SC2029 # $rev is meant to expand locally.
ssh "$host" "set -e
    chmod 755 /opt/discord-bot/discord-bot.new
    mv /opt/discord-bot/discord-bot.new /opt/discord-bot/discord-bot
    echo $rev > /opt/discord-bot/revision
    systemctl restart discord-bot.service
    sleep 5
    journalctl -u discord-bot.service --no-pager -o cat -n 10
    systemctl is-active --quiet discord-bot.service"
echo "Deployed $rev to $host"
