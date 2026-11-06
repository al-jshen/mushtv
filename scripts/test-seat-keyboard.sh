#!/usr/bin/env bash
# Checks the launcher's seat keyboard fix (launcher/src/seat_keyboard.rs) in a
# headless cage: a keyboard comes and goes, which leaves cage without a seat
# keyboard, and an app started after a rearm must still get a keyboard layout
# (VacuumTube crashes without one). No root needed; changes nothing.
#
# Usage: scripts/test-seat-keyboard.sh
set -euo pipefail

cd "$(dirname "$0")/../launcher"
# Build outside cage, so the run inside it is quick.
cargo test --release --no-run -q

result=$(mktemp)
trap 'rm -f "$result"' EXIT
WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 120 cage -- sh -c \
  'cargo test --release -q seat_keyboard -- --ignored >"$1" 2>&1; echo "exit $?" >>"$1"' \
  sh "$result" 2>/dev/null || true
cat "$result"
grep -qx "exit 0" "$result"
