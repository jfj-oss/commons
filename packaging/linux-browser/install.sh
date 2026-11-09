#!/usr/bin/env bash
# Puts Megatron (browser version) in your app menu. Run it from the unpacked folder:  ./install.sh
set -eu
SRC="$(cd "$(dirname "$0")" && pwd)"
DST="${XDG_DATA_HOME:-$HOME/.local/share}/megatron-browser"
[ -f "$DST/.pid" ] && kill "$(cat "$DST/.pid")" 2>/dev/null || true
mkdir -p "$DST" "$HOME/.local/bin" "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
rm -rf "$DST/ui" "$DST/.update-previous" "$DST/.update-note.json" "$DST/.update.lock"
cp -r "$SRC/ui" "$SRC/bridge" "$SRC/megatron" "$SRC/relay.txt" "$SRC/megatron.png" "$DST/"
chmod +x "$DST/bridge" "$DST/megatron"
ln -sf "$DST/megatron" "$HOME/.local/bin/megatron"
cat > "${XDG_DATA_HOME:-$HOME/.local/share}/applications/megatron-browser.desktop" <<D
[Desktop Entry]
Type=Application
Name=mega
Comment=Private group chat, in your browser window
Exec=$DST/megatron
Icon=$DST/megatron.png
Terminal=false
Categories=Network;Chat;
D
command -v update-desktop-database >/dev/null && update-desktop-database "${XDG_DATA_HOME:-$HOME/.local/share}/applications" 2>/dev/null || true
echo "Installed. Open mega from your app menu (or run: megatron)."
