#!/usr/bin/env bash
# Installs mega (native) for your user: the program, its icons and an app-menu entry. Run from the unpacked folder:
#   ./install.sh [--prefix DIR]        default DIR: ~/.local   (needs no root)
# Your account is not touched: mega uses the same folder as the older Megatron app (~/.local/share/app.commons.chat).
set -eu
SRC="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${HOME}/.local"
[ "${1:-}" = "--prefix" ] && { PREFIX="${2:?--prefix needs a folder}"; }
BIN="$PREFIX/bin"
DATA="${XDG_DATA_HOME:-$PREFIX/share}"
[ "$PREFIX" = "$HOME/.local" ] && DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS="$DATA/applications"
ID=app.commons.chat.gpui
[ -x "$SRC/mega-gpui" ] || { echo "mega-gpui is not next to install.sh" >&2; exit 1; }
mkdir -p "$BIN" "$APPS"
# replace the program with a new file so a running mega keeps its old one until it restarts (cp over a running binary fails)
cp "$SRC/mega-gpui" "$BIN/.mega-gpui.new" && chmod 755 "$BIN/.mega-gpui.new" && mv -f "$BIN/.mega-gpui.new" "$BIN/mega-gpui"
for f in "$SRC"/icons/*.png; do
  s=$(basename "$f" .png)
  mkdir -p "$DATA/icons/hicolor/${s}x${s}/apps"
  cp "$f" "$DATA/icons/hicolor/${s}x${s}/apps/$ID.png"
done
# the icon by absolute path too: a desktop that does not read the hicolor folder still finds it
mkdir -p "$DATA/mega"
cp "$SRC/icons/256.png" "$DATA/mega/mega.png"
cat > "$APPS/$ID.desktop" <<D
[Desktop Entry]
Type=Application
Name=mega
Comment=Private group chat
Exec=$BIN/mega-gpui %u
Icon=$DATA/mega/mega.png
Terminal=false
Categories=Network;Chat;InstantMessaging;
StartupWMClass=$ID
StartupNotify=true
D
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
echo "Installed mega to $BIN/mega-gpui and added it to your app menu."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Note: $BIN is not on your PATH; the app menu entry works anyway." ;; esac
echo "It opens the same account as the older Megatron app. Close that app first: only one can have the account open."
