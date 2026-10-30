#!/usr/bin/env bash
# Tests deploy/wifi-switch-to-iwd.sh's parsing against fixtures. Changes
# nothing and needs no root: only its --print-* modes run.
# Usage: scripts/test-wifi-switch.sh
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
switch="$repo/deploy/wifi-switch-to-iwd.sh"
fx="$repo/scripts/fixtures"
fails=0
check() {  # description, expected, actual
  if [ "$2" = "$3" ]; then echo "ok   - $1"; else
    echo "FAIL - $1"; echo "  expected: $(printf %q "$2")"; echo "  actual:   $(printf %q "$3")"
    fails=$((fails + 1))
  fi
}
iwd_file() { bash "$switch" --print-iwd-file "$fx/$1" 2>&1; }
nl=$'\n'

check "passphrase: name and Passphrase=" \
  "Home Net.psk${nl}[Security]${nl}Passphrase=correct horse battery" "$(iwd_file interfaces-passphrase)"
check "64 hex digits with --hex-ok: PreSharedKey=" \
  "HomeNet.psk${nl}[Security]${nl}PreSharedKey=0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef" \
  "$(bash "$switch" --hex-ok --print-iwd-file "$fx/interfaces-hex" 2>&1)"
if out=$(iwd_file interfaces-hex); then
  check "64 hex digits without --hex-ok are refused" "non-zero exit" "exit 0: $out"
else
  check "64 hex digits without --hex-ok are refused, naming --hex-ok" "yes" \
    "$(grep -q -- '--hex-ok' <<<"$out" && echo yes || echo "no: $out")"
fi
check "a backslash is escaped" \
  "Home.psk${nl}[Security]${nl}Passphrase=back\\\\slash12" "$(iwd_file interfaces-backslash)"
check "a leading space is escaped as \\s" \
  "Home.psk${nl}[Security]${nl}Passphrase=\\slead12345" "$(iwd_file interfaces-leading-space)"
check "a hidden network is marked Hidden" \
  "Home.psk${nl}[Security]${nl}Passphrase=secret123${nl}[Settings]${nl}Hidden=true" "$(iwd_file interfaces-hidden)"
check "odd SSID: = and lower-case hex" \
  "=636166c3a921.psk${nl}[Security]${nl}Passphrase=s3cret pass" "$(iwd_file interfaces-odd-ssid)"
check "quoted values are unquoted" \
  "Home Net.psk${nl}[Security]${nl}Passphrase=correct horse battery" "$(iwd_file interfaces-quoted)"
check "other stanzas' wpa-* lines are ignored" \
  "Home.psk${nl}[Security]${nl}Passphrase=thisone123" "$(iwd_file interfaces-other-stanzas)"

if out=$(bash "$switch" --print-iwd-file "$fx/interfaces-no-wifi" 2>&1); then
  check "no wlp2s0 stanza fails" "non-zero exit" "exit 0: $out"
else
  check "no wlp2s0 stanza fails with a clear message" "yes" \
    "$(grep -q 'no wlp2s0 stanza' <<<"$out" && echo yes || echo "no: $out")"
fi

edited="$(bash "$switch" --print-edited-interfaces "$fx/interfaces-other-stanzas")"
expected="$(sed -e 's/^    wpa-ssid Home$/# moved to iwd:     wpa-ssid Home/' \
  -e 's/^    wpa-psk thisone123$/# moved to iwd:     wpa-psk thisone123/' "$fx/interfaces-other-stanzas")"
check "only wlp2s0's wpa-* lines are commented out" "$expected" "$edited"
check "the rest of the file is kept" "$(grep -c . "$fx/interfaces-other-stanzas")" "$(grep -c . <<<"$edited")"

if [ "$fails" -ne 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all wifi-switch checks passed"
