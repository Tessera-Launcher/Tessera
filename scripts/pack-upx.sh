#!/usr/bin/env bash
# Compression for the Linux artifact. Mirrors pack-upx.ps1: UPX is pinned so
# the same executable is produced on every run.
set -euo pipefail

path="${1:?usage: pack-upx.sh <binary>}"
version='5.3.1'
sha256='a559745c9de96101f77cab4a71c396556181b748fcade73e51cc5a6268dcecad'

root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
dir="$root/upx-$version"
archive="$dir/upx-$version-amd64_linux.tar.xz"
mkdir -p "$dir"

if [ ! -f "$archive" ]; then
    curl -fsSL -o "$archive" \
        "https://github.com/upx/upx/releases/download/v$version/upx-$version-amd64_linux.tar.xz"
fi

actual="$(sha256sum "$archive" | cut -d' ' -f1)"
if [ "$actual" != "$sha256" ]; then
    echo "UPX archive checksum mismatch: expected $sha256, got $actual" >&2
    exit 1
fi

tar -xJf "$archive" -C "$dir"
upx="$(find "$dir" -type f -name upx | head -1)"
if [ -z "$upx" ]; then
    echo "upx binary not found in $archive" >&2
    exit 1
fi
chmod +x "$upx"

# The build cache can restore a binary that an earlier run already packed, and
# UPX refuses to pack a file twice. Packed executables carry the `UPX!` marker,
# so detect that and leave the file alone.
if grep -qa 'UPX!' "$path"; then
    echo "UPX: $path is already packed, skipping"
    exit 0
fi

before="$(stat -c%s "$path")"
"$upx" --best --lzma "$path"
"$upx" -t "$path"
after="$(stat -c%s "$path")"
echo "UPX: $before -> $after bytes"
