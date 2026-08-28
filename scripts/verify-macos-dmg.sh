#!/usr/bin/env bash

set -euo pipefail

if [[ $(uname -s) != "Darwin" ]]; then
  echo "verify-macos-dmg requires macOS." >&2
  exit 2
fi
if [[ $# -ne 1 || ! -f $1 ]]; then
  echo "Usage: scripts/verify-macos-dmg.sh <path-to-dmg>" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "$script_dir/.." && pwd)"
dmg_path="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
mount_dir="$(mktemp -d -t cleanerx-dmg-verify)"
mounted=false

cleanup() {
  if [[ $mounted == true ]]; then
    hdiutil detach "$mount_dir" >/dev/null 2>&1 || true
  fi
  rmdir "$mount_dir" >/dev/null 2>&1 || true
}
trap cleanup EXIT

hdiutil verify "$dmg_path" >/dev/null
hdiutil attach -readonly -nobrowse -mountpoint "$mount_dir" "$dmg_path" >/dev/null
mounted=true

node "$repository_root/scripts/verify-macos-dmg-layout.mjs" \
  "$mount_dir" \
  "$repository_root/assets/branding/dmg-background.png"

hdiutil detach "$mount_dir" >/dev/null
mounted=false
rmdir "$mount_dir"
trap - EXIT
