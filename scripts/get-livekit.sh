#!/usr/bin/env bash
# Puts livekit-server into tools/bin (git-ignored), checked against the release's published checksum. Linux x86-64.
set -euo pipefail
cd "$(dirname "$0")/.."
V=1.13.11
[ -x tools/bin/livekit-server ] && { echo "tools/bin/livekit-server is already there"; exit 0; }
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
U=https://github.com/livekit/livekit/releases/download/v$V
curl -fsSL -o "$T/lk.tgz" "$U/livekit_${V}_linux_amd64.tar.gz"
curl -fsSL -o "$T/sums" "$U/checksums.txt"
want=$(awk -v f="livekit_${V}_linux_amd64.tar.gz" '$2==f {print $1}' "$T/sums")
got=$(sha256sum "$T/lk.tgz" | cut -d' ' -f1)
[ -n "$want" ] && [ "$want" = "$got" ] || { echo "checksum of the LiveKit download does not match; not installing it" >&2; exit 1; }
mkdir -p "$T/x" tools/bin
tar -xzf "$T/lk.tgz" -C "$T/x"
install -m 755 "$T/x/livekit-server" tools/bin/livekit-server
tools/bin/livekit-server --version
