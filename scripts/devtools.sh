#!/usr/bin/env bash
# Fetch the headless-testing tools (grim, wtype, wlr-randr) as Debian packages
# and unpack them into launcher/.devtools/root. No root needed, nothing installed.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)/launcher/.devtools"
rm -rf "$root"
mkdir -p "$root/debs" "$root/root"
cd "$root/debs"
apt download grim wtype wlr-randr
for deb in *.deb; do dpkg -x "$deb" "$root/root"; done
ls "$root/root/usr/bin"
