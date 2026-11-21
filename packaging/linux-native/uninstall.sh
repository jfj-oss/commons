#!/usr/bin/env bash
# Removes mega (native): the program, icons and the app-menu entry. Your account and messages
# (~/.local/share/app.commons.chat) are NOT touched. Same --prefix as install.sh if you used one.
set -eu
PREFIX="${HOME}/.local"
[ "${1:-}" = "--prefix" ] && { PREFIX="${2:?--prefix needs a folder}"; }
BIN="$PREFIX/bin"
DATA="${XDG_DATA_HOME:-$PREFIX/share}"
[ "$PREFIX" = "$HOME/.local" ] && DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ID=app.commons.chat.gpui
rm -f "$BIN/mega-gpui" "$BIN/mega-gpui.previous" "$DATA/applications/$ID.desktop"
rm -f "$DATA"/icons/hicolor/*/apps/"$ID".png
rm -rf "$DATA/mega"
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
command -v update-desktop-database >/dev/null && update-desktop-database "$DATA/applications" 2>/dev/null || true
echo "mega removed. Your account folder was left alone."
