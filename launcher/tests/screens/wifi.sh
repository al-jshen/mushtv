# Settings → Wi-Fi and the password keyboard, on canned data. Run with demo
# mode, so nothing talks to BlueZ, PipeWire or iwd:
#   MUSHTV_DEMO=1 scripts/headless.sh launcher/launcher.toml <outdir> launcher/tests/screens/wifi.sh
# Tiles: three apps, Mirroring, Settings (one row).
key Right Right Right Right Return
pause 1
key Down Down
shot settings-menu
key Return
pause 1
shot wifi-list
key Return                      # Home: its network page
pause 0.5
shot wifi-network
key Escape Down Return          # Marriott-Guest: the keyboard
pause 0.5
shot keyboard
type_text "Hunter 22"
pause 0.5
shot keyboard-typed             # 9 dots, Connect highlighted
key Escape Return               # discard, reopen
key Down Down Down Left Return  # q → a → z → Symbols, then Shift
pause 0.5
shot keyboard-shift
key Right Return                # the symbols layout
pause 0.5
shot keyboard-symbols
key Up Return Return Down Right Right Right Return  # type '""', then Show
pause 0.5
shot keyboard-shown
key Escape Return               # discard, reopen
type_text "abc"
key Return                      # Connect with 3 characters: the toast
pause 0.5
shot keyboard-toast             # the toast sits above the keyboard
