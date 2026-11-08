#!/usr/bin/env bash
# Install and start the megatron-bot systemd *user* service. Run this yourself; nothing enables autostart on its own.
#   packaging/megatron-bot/enable.sh
# It does not need root. Stop it again with: systemctl --user disable --now megatron-bot
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bin="${MEGATRON_BOT_BIN:-$HOME/.local/bin/megatron-bot}"
data="${MEGATRON_BOT_DATA:-$HOME/.local/share/megatron-bot}"

if [ ! -x "$bin" ]; then
  cat >&2 <<EOF
megatron-bot is not installed at $bin.
Build and install it, then run this script again:
  cargo build --release -p commons-bot
  install -Dm755 target/release/megatron-bot "$bin"
EOF
  exit 1
fi

if [ ! -f "$data/state.cmns" ]; then
  cat >&2 <<EOF
There is no bot account in $data yet.
Run it once to create the account and join a community:
  megatron-bot join <invite-link>
EOF
  exit 1
fi

mkdir -p "$HOME/.config/systemd/user"
install -m644 "$here/megatron-bot.service" "$HOME/.config/systemd/user/megatron-bot.service"
systemctl --user daemon-reload
systemctl --user enable --now megatron-bot.service
echo "megatron-bot is running. Follow its log with:"
echo "  journalctl --user -u megatron-bot -f"
