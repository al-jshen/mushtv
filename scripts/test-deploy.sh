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
wp="$s/home/tv/.config/wireplumber/wireplumber.conf.d/51-mushtv-hdmi.conf"
check "HDMI audio rule installed 644" test "$(stat -c %a "$wp" 2>/dev/null)" = 644
check "iwd D-Bus policy installed 644" test "$(stat -c %a "$s/etc/dbus-1/system.d/mushtv-iwd.conf" 2>/dev/null)" = 644
check "iwd main.conf installed 644" test "$(stat -c %a "$s/etc/iwd/main.conf" 2>/dev/null)" = 644
check "iwd keeps the kernel's interface" grep -qx 'DefaultInterface=\*' "$s/etc/iwd/main.conf"
check "iwd's interface renaming masked" test "$(readlink "$s/etc/systemd/network/80-iwd.link")" = /dev/null
check "8BitDo mouse rule installed 644" test "$(stat -c %a "$s/etc/udev/rules.d/70-mushtv-8bitdo-mouse.rules" 2>/dev/null)" = 644
check "libinput quirk installed 644" test "$(stat -c %a "$s/etc/libinput/local-overrides.quirks" 2>/dev/null)" = 644

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

# Jellyfin's Game Controller setting is turned on; its other settings stay.
jf_rel=home/tv/.var/app/com.github.iwalton3.jellyfin-mpv-shim/config/jellyfin-mpv-shim/conf.json
check "no Jellyfin settings made before Jellyfin made its own" test ! -e "$work/fresh/$jf_rel"
s="$work/jellyfin"
jf="$s/$jf_rel"
mkdir -p "$(dirname "$jf")"
printf '{\n    "config_version": 6,\n    "input_gamepad": false,\n    "video_volume": 80\n}' > "$jf"
deploy "$s"
check "Jellyfin controller turned on, other settings kept" python3 -I -c '
import json, sys
c = json.load(open(sys.argv[1]))
sys.exit(not (c == {"config_version": 6, "input_gamepad": True, "video_volume": 80}))' "$jf"
check "deploy says it turned it on" grep -q 'turned on Game Controller' "$work/out.log"
deploy "$s"
check "already on: nothing said" bash -c "! grep -q 'Game Controller' '$work/out.log'"

# ...but not while Jellyfin is open: it writes its own settings back.
printf '{"input_gamepad": false}' > "$jf"
bash -c 'exec -a jellyfin-mpv-shim sleep 30' &
jf_pid=$!
until pgrep -u "$(id -u)" -f jellyfin-mpv-shim >/dev/null; do sleep 0.1; done
deploy "$s"
kill "$jf_pid"
check "left alone while Jellyfin is open" test "$(cat "$jf")" = '{"input_gamepad": false}'
check "says to close Jellyfin first" grep -q 'close Jellyfin' "$work/out.log"

# A settings file that isn't JSON is left alone, and deploy still finishes.
printf 'not json' > "$jf"
check "deploy finishes with unreadable Jellyfin settings" deploy "$s"
check "unreadable Jellyfin settings untouched" test "$(cat "$jf")" = "not json"

if [ "$fails" -ne 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all deploy checks passed"
