#!/usr/bin/env bash
# Run the launcher inside a headless cage and drive it from a scenario script.
#
# Usage: scripts/headless.sh <config.toml> <outdir> <scenario.sh> [WIDTHxHEIGHT]
#
# The scenario is sourced inside cage, after the launcher has started, with:
#   shot <name>      screenshot to <outdir>/<name>.png
#   key <keysym>...  press and release keys via a virtual keyboard (e.g. Right, Return)
#   pause <seconds>  sleep
#   $LAUNCHER_PID    pid of the launcher
# Needs scripts/devtools.sh to have been run once.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
config="$(realpath "$1")"
outdir="$(realpath -m "$2")"
scenario="$(realpath "$3")"
mode="${4:-1920x1080}"
tools="$repo/launcher/.devtools/root/usr/bin"
bin="${LAUNCHER_BIN:-$repo/launcher/target/release/mushtv-launcher}"

[ -x "$tools/grim" ] || { echo "run scripts/devtools.sh first" >&2; exit 1; }
mkdir -p "$outdir"

inner=$(mktemp)
trap 'rm -f "$inner"' EXIT
cat > "$inner" <<EOF
set -u
export PATH="$tools:\$PATH"
shot() { grim "$outdir/\$1.png"; }
key() { for k in "\$@"; do wtype -k "\$k"; sleep 0.3; done; }
pause() { sleep "\$1"; }
wlr-randr --output HEADLESS-1 --custom-mode "$mode" >/dev/null 2>&1 || echo "headless: could not set mode $mode" >&2
# Headless cage has no keyboard. Keep a virtual one alive for the whole run,
# so the launcher has a wl_keyboard before the first scripted key press.
wtype -s 600000 &
KEYBOARD_PID=\$!
sleep 0.3
"$bin" --config "$config" &
LAUNCHER_PID=\$!
sleep 2
. "$scenario"
kill \$LAUNCHER_PID 2>/dev/null
wait \$LAUNCHER_PID 2>/dev/null
kill \$KEYBOARD_PID 2>/dev/null
EOF

WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 120 cage -- bash "$inner"
