#!/usr/bin/env bash
# Tests deploy.sh with DESTDIR staging (no root needed).
# Usage: scripts/test-deploy.sh   (needs a release build of the launcher)
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
check() {  # description, then a command that must succeed
  local what="$1"; shift
  if "$@"; then echo "ok   - $what"; else echo "FAIL - $what"; fails=$((fails + 1)); fi
}
deploy() { DESTDIR="$1" "$repo/deploy.sh" > "$work/out.log" 2>&1; }

# Fresh install lands every file with the right mode.
s="$work/fresh"
deploy "$s"
check "binary installed 755" test "$(stat -c %a "$s/usr/local/bin/mushtv-launcher")" = 755
check "config installed 644" test "$(stat -c %a "$s/etc/mushtv/launcher.toml")" = 644
check "autologin installed" test -f "$s/etc/systemd/system/getty@tty1.service.d/autologin.conf"
check "bash_profile installed" test -f "$s/home/tv/.bash_profile"

# A second run changes nothing.
deploy "$s"
check "second run is a no-op" bash -c "! grep -q updating '$work/out.log'"

# tv's home keeps its permissions.
s="$work/private-home"
mkdir -p "$s/home/tv"
chmod 700 "$s/home/tv"
deploy "$s"
check "existing home dir stays 700" test "$(stat -c %a "$s/home/tv")" = 700

# Symlinks planted in tv's home are replaced, never written through.
s="$work/symlinks"
mkdir -p "$s/home/tv"
echo original > "$work/victim1"
echo original > "$work/victim2"
echo "old profile" > "$s/home/tv/.bash_profile"
ln -s "$work/victim1" "$s/home/tv/.bash_profile.bak"
deploy "$s"
check "backup symlink not written through" test "$(cat "$work/victim1")" = original
check "backup is a regular file" test -f "$s/home/tv/.bash_profile.bak" -a ! -L "$s/home/tv/.bash_profile.bak"
rm -f "$s/home/tv/.bash_profile"
ln -s "$work/victim2" "$s/home/tv/.bash_profile"
deploy "$s"
check "profile symlink not written through" test "$(cat "$work/victim2")" = original
check "profile is now a regular file" test -f "$s/home/tv/.bash_profile" -a ! -L "$s/home/tv/.bash_profile"

if [ "$fails" -ne 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all deploy checks passed"
