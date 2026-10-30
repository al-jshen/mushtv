#!/usr/bin/env bash
# One-time switch of the TV box's Wi-Fi from wpa_supplicant (started by
# ifupdown) to iwd, so the launcher's Settings can scan for and join
# networks. Run it yourself, with sudo, while nothing is streaming:
#
#   sudo bash deploy/wifi-switch-to-iwd.sh --dry-run   show what it would do
#   sudo bash deploy/wifi-switch-to-iwd.sh             do it
#
# If wpa-psk is a 64-digit key rather than the password, iwd can use it only
# on WPA2 routers: add --hex-ok if yours is WPA2-only. Otherwise switch
# without it to see the error, and after switching join home Wi-Fi again
# from Settings with the real password.
#
# Only the Wi-Fi changes hands: DHCP for wlp2s0 stays with ifupdown and
# dhcpcd (metric 600), and Ethernet (enp3s0), Docker and Tailscale are left
# alone. The home network moves to iwd, so the box rejoins it by itself.
# Undo: README.md, "Switching to iwd (once)".
#
# For tests (no root, changes nothing; --hex-ok may come first):
#   --print-iwd-file <interfaces>            the iwd file for wlp2s0's network
#   --print-edited-interfaces <interfaces>   the file with wlp2s0's wpa-* lines
#                                            commented out
set -euo pipefail
export LC_ALL=C

iface=wlp2s0
ethernet=enp3s0
interfaces=/etc/network/interfaces
iwd_dir=/var/lib/iwd
repo="$(cd "$(dirname "$0")/.." && pwd)"

die() { echo "wifi-switch-to-iwd: $*" >&2; exit 1; }

# A stanza runs from its iface line to the next stanza keyword.
STANZA='/^[[:space:]]*(iface|auto|allow-[^[:space:]]*|source|source-directory|mapping)([[:space:]]|$)/'

# The value of option $2 in wlp2s0's stanza of file $1, unquoted.
option() {
  awk -v ifc="$iface" -v key="$2" "
    $STANZA { inside = (\$1 == \"iface\" && \$2 == ifc) }
    inside && \$1 == key {
      sub(/^[[:space:]]*[^[:space:]]+[[:space:]]+/, \"\")
      sub(/[[:space:]]+\$/, \"\")
      print
      exit
    }" "$1" | sed -e 's/^"\(.*\)"$/\1/'
}

# iwd names a network's file after its SSID if that's only letters, digits,
# spaces, '_' and '-'; otherwise '=' and the SSID in lower-case hex.
iwd_name() {
  if [[ "$1" =~ ^[A-Za-z0-9\ _-]+$ ]]; then
    printf '%s.psk\n' "$1"
  else
    printf '=%s.psk\n' "$(printf '%s' "$1" | od -An -tx1 | tr -d ' \n')"
  fi
}

# A value as iwd's settings files want it: backslashes doubled, and a
# leading space written \s.
iwd_escape() {
  local v="${1//\\/\\\\}"
  [ "${v:0:1}" = " " ] && v="\\s${v:1}"
  printf '%s\n' "$v"
}

# The iwd file for wlp2s0's network: its name, then its content.
iwd_file() {
  local ssid psk
  grep -Eq "^[[:space:]]*iface[[:space:]]+$iface[[:space:]]" "$1" \
    || die "no $iface stanza in $1"
  ssid="$(option "$1" wpa-ssid)"
  psk="$(option "$1" wpa-psk)"
  [ -n "$ssid" ] || die "no wpa-ssid in the $iface stanza of $1"
  [ -n "$psk" ] || die "no wpa-psk in the $iface stanza of $1"
  if [[ "$psk" =~ ^[0-9A-Fa-f]{64}$ ]] && ! $hex_ok; then
    die "wpa-psk for $iface is a 64-digit key, not the password. iwd can use a key only on WPA2-only routers; with WPA3 (or WPA2/WPA3) the box wouldn't rejoin home Wi-Fi. If your router is WPA2-only, run again with --hex-ok. Otherwise put the real password in wpa-psk first, or switch with --hex-ok and then join home Wi-Fi again from Settings."
  fi
  iwd_name "$ssid"
  echo "[Security]"
  if [[ "$psk" =~ ^[0-9A-Fa-f]{64}$ ]]; then
    echo "PreSharedKey=$psk"
  else
    echo "Passphrase=$(iwd_escape "$psk")"
  fi
  # A hidden network has to be marked, or iwd won't look for it.
  if [ "$(option "$1" wpa-scan-ssid)" = 1 ]; then
    echo "[Settings]"
    echo "Hidden=true"
  fi
}

# File $1 with the wpa-* lines of wlp2s0's stanza commented out.
edited_interfaces() {
  awk -v ifc="$iface" "
    $STANZA { inside = (\$1 == \"iface\" && \$2 == ifc) }
    inside && \$1 ~ /^wpa-/ { print \"# moved to iwd: \" \$0; next }
    { print }" "$1"
}

hex_ok=false
dry=false
while [ $# -gt 0 ]; do
  case "$1" in
    --hex-ok) hex_ok=true ;;
    --print-iwd-file) iwd_file "${2:?usage: --print-iwd-file <interfaces>}"; exit ;;
    --print-edited-interfaces) edited_interfaces "${2:?usage: --print-edited-interfaces <interfaces>}"; exit ;;
    --dry-run) dry=true ;;
    *) die "unknown argument $1 (see the top of this script)" ;;
  esac
  shift
done

[ "$(id -u)" -eq 0 ] || die "run with sudo"
step() { echo "== $*"; }
run() {  # a command, or just its description in a dry run
  if $dry; then echo "   would run: $*"; else "$@"; fi
}

step "checking Ethernet ($ethernet) carries the default route"
ip -4 route show default dev "$ethernet" | grep -q . \
  || die "$ethernet has no default route. Plug in Ethernet first, so the box stays reachable if Wi-Fi drops."
grep -Eq "^[[:space:]]*iface[[:space:]]+$iface[[:space:]]" "$interfaces" \
  || die "no $iface stanza in $interfaces; nothing to switch"
if ! grep -Eq '^[[:space:]]*wpa-' <(awk -v ifc="$iface" "$STANZA { inside = (\$1 == \"iface\" && \$2 == ifc) } inside" "$interfaces"); then
  die "$iface has no wpa-* lines in $interfaces: already switched?"
fi
network="$(iwd_file "$interfaces")"
name="$(head -n 1 <<<"$network")"

backup="$interfaces.bak-$(date +%Y%m%d-%H%M%S)"
step "backing up $interfaces to $backup"
run cp -p "$interfaces" "$backup"

step "installing iwd and mushtv's iwd settings"
run env DEBIAN_FRONTEND=noninteractive apt-get install -y -o Dpkg::Options::=--force-confold iwd
run install -D -m 644 "$repo/deploy/iwd-main.conf" /etc/iwd/main.conf
run install -D -m 644 "$repo/deploy/dbus-mushtv-iwd.conf" /etc/dbus-1/system.d/mushtv-iwd.conf
run install -d -m 755 /etc/systemd/network
run ln -sfn /dev/null /etc/systemd/network/80-iwd.link

step "moving the home network to iwd: $iwd_dir/$name"
if ! $dry; then
  install -d -m 700 "$iwd_dir"
  (umask 077 && tail -n +2 <<<"$network" > "$iwd_dir/$name")
fi

step "taking $iface down and removing its wpa-* lines from $interfaces"
run ifdown "$iface"
if ! $dry; then
  (umask 077 && edited_interfaces "$interfaces" > "$interfaces.new")
  chmod --reference="$interfaces" "$interfaces.new"
  mv "$interfaces.new" "$interfaces"
fi

step "replacing wpa_supplicant with iwd, then bringing $iface up"
run systemctl disable --now wpa_supplicant
run systemctl enable --now iwd
run sleep 3
run ifup "$iface"

if $dry; then
  echo "dry run: nothing changed"
  exit 0
fi
step "result"
iwctl station "$iface" show || true
ip route
echo "done. To undo, see README.md, \"Switching to iwd (once)\"."
