#!/usr/bin/env bash
# Build the static musl release binary and deploy it to a ZFS host (PVE),
# installing it as a systemd service. Read-only host-side: only writes the
# binary, .env, and the unit (binary + env live under /usr/local/bin to match
# the running service).
set -euo pipefail

# Target host (user@host) and optional explicit binary path.
HOST="${HOST:-root@pve}"
BIN_SRC="target/x86_64-unknown-linux-musl/release/zfs-ha-monitor"
DEST_DIR="/usr/local/bin"
SERVICE="zfs-ha-monitor"

die() { echo "ERROR: $*" >&2; exit 1; }

command -v cargo >/dev/null || die "cargo not installed"
command -v ssh  >/dev/null || die "ssh not installed"
command -v scp  >/dev/null || die "scp not installed"
[ -f .env ] || die ".env missing: cp .env.example .env and configure it"

echo ">>> Building static musl release..."
cargo build --release --target x86_64-unknown-linux-musl

[ -f "$BIN_SRC" ] || die "binary not found: $BIN_SRC"

echo ">>> Deploying to $HOST..."
ssh "$HOST" "mkdir -p '$DEST_DIR'"
scp "$BIN_SRC" "$HOST:$DEST_DIR/zfs-ha-monitor"
scp .env "$HOST:$DEST_DIR/.env"
scp packaging/zfs-ha-monitor.service "$HOST:/etc/systemd/system/$SERVICE.service"

ssh "$HOST" "chmod +x '$DEST_DIR/zfs-ha-monitor' && systemctl daemon-reload && systemctl enable --now $SERVICE && systemctl --no-pager status $SERVICE --lines=0"

echo ">>> Deployed. Service: $SERVICE on $HOST"
echo ">>> To restart:  systemctl restart $SERVICE"
echo ">>> To view logs: journalctl -u $SERVICE -f"
