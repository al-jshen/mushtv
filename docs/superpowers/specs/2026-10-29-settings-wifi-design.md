# Settings: Wi-Fi with an on-screen keyboard (design)

Date: 2026-10-29. This extends `2026-10-14-settings-audio-bluetooth-design.md`. The screens below were agreed in conversation. Choices the user didn't make are marked **(my call)**.

## Intent

The user takes the TV box away from home, for example to a hotel. There's no Ethernet there, but there is Wi-Fi. They get the box online from the TV itself, with the remote, a controller or a keyboard. They can't SSH in or use another computer first, because the box isn't online yet.

## Goals

- **Scan and join:** from Settings, scan for Wi-Fi networks and join an open or password (WPA-Personal) network.
- **Type the password with either hand:** an on-screen keyboard works with a controller or the remote, and also takes typing from a physical keyboard. Both can be mixed.
- **Remember networks:** joined networks are remembered, and the box reconnects to them by itself when they're in range.
- **Manage saved networks:** see the current network, and disconnect or forget a saved one.
- **Ethernet first:** when Ethernet is plugged in, it stays the box's main connection. Wi-Fi never takes over from it.

## Non-goals

- **Hotel sign-in pages:** the box has no browser, so a network that needs a sign-in page connects but nothing loads. This limitation is documented in the README. **(my call; I raised it and the user didn't ask for it)**
- **Other network types:** hidden networks (no name), enterprise networks (802.1X, username and password) and WEP. Enterprise networks are listed as "Not supported" so they don't look broken. **(my call)**
- **AirPlay on hotel Wi-Fi:** hotel networks often stop devices from seeing each other, which blocks AirPlay. Not solved here.
- **DNS away from home** (see Risks).
- **Ethernet settings, static IPs, and running the box as a Wi-Fi hotspot.**

## Anti-goals

These would count as a failure even if Wi-Fi joining worked:

- **Breaking the home server:** the box's Ethernet, Docker, Tailscale or server workloads stop working because of this change.
- **Network changes outside the user's control:** anything changes the box's network setup while the user is streaming, or without the user running the step themselves (see Rollout).
- **Leaking the password:** a Wi-Fi password lands in a log, in the launcher's own files, or in the repo.
- **Losing home Wi-Fi:** the box no longer joins the home Wi-Fi after the switch.

## Constraints

- **No restarts or network-affecting tests during the build.** The user is playing PlateUp! through Moonlight on this box. The builder must not:
  - restart the launcher, cage, any service or any network interface;
  - install packages;
  - run `deploy.sh`;
  - talk to the real BlueZ, iwd or wpa_supplicant from tests or headless runs.

  Building and unit tests are fine. So are headless screenshots, but only in a fake-data mode (see Testing).
- **Same style as Bluetooth:** same look, page model (`settings.rs` state machine), worker-thread pattern (`bluetooth/`), keys and toasts.
- **Commits:** the user commits (GPG signing); the builder hands over commit commands.

## Verified facts (2026-10-29, on the box)

- **Today's Wi-Fi setup:**
  - ifupdown (`networking` active) brings up `wlp2s0` through its wpa_supplicant hook. The process is `wpa_supplicant -s -B -P /run/wpa_supplicant.wlp2s0.pid -i wlp2s0 -D nl80211,wext -C /run/wpa_supplicant`.
  - There is no config file, so the SSID and password live in `/etc/network/interfaces` (root-only, mode 600).
  - A second, D-Bus-only `wpa_supplicant -u` (`wpa_supplicant.service`) also runs, managing no interface.
  - No NetworkManager, iwd, systemd-networkd or connman.
- **Routes:**
  - Ethernet: `default via 10.20.188.1 dev enp3s0 … metric 100`.
  - Wi-Fi: `default via … dev wlp2s0 … metric 600`.
  - DHCP is done by ifupdown with `dhcpcd-base`, so the metrics come from `/etc/network/interfaces`.
- **Hardware:** the Wi-Fi card is the Realtek RTL8821CE (`rtw_8821ce` driver), the same combo chip as Bluetooth.
- **DNS:** Tailscale writes `/etc/resolv.conf` (`nameserver 100.100.100.100`, MagicDNS). `systemd-resolved` and `resolvconf` aren't installed.
- **iwd 3.8-2 is in Debian 13 but not installed.** Its package:
  - **D-Bus policy:** allows the `sudo` and `netdev` groups to call `net.connman.iwd`, and root to call `net.connman.iwd.Agent` on clients. `tv` is in neither group; `mushy` is in both.
  - **Service:** runs on demand by default; `systemctl enable --now iwd` makes it persistent.
  - **Interface renaming:** it ships `/usr/lib/systemd/network/80-iwd.link` (`NamePolicy=keep kernel`), which would rename `wlp2s0` to `wlan0` on the next boot and break the ifupdown stanza.
  - **Network configuration:** off by default (`EnableNetworkConfiguration` defaults to false), so iwd only associates and leaves DHCP and routes alone.
  - **Interface re-creation:** at startup iwd deletes the kernel's wireless interface and creates its own, named `wlan<N>` (src/manager.c). It skips this only for drivers matched by `[DriverQuirks] DefaultInterface` (or the deprecated `UseDefaultInterface`). Left alone, this would rename `wlp2s0` underneath ifupdown and dhcpcd.
- **iwd's API** (doc/ at tag 3.8):
  - **Station** (`/net/connman/iwd/phyN/M`): `Scan()`, `Disconnect()`, `GetOrderedNetworks() -> a(on)` and the `State`, `Scanning` and `ConnectedNetwork` properties.
    - `GetOrderedNetworks()` returns network paths with signal in dBm×100; the connected network is first, then known, then others.
    - `Disconnect()` also stops autoconnect on that device until the next `Connect()`.
  - **Network:**
    - `Connect()` can fail with `Aborted`, `Busy`, `Failed`, `NoAgent`, `NotSupported`, `Timeout`, `InProgress` or `NotConfigured`.
    - Properties: `Name`, `Connected`, `Type` ("open", "wep", "psk", "8021x") and an optional `KnownNetwork`.
  - **KnownNetwork:** `Forget()`.
  - **AgentManager** (`/net/connman/iwd`): `RegisterAgent(o)`.
  - **Agent** (`net.connman.iwd.Agent`): `RequestPassphrase(o network) -> s`, cancellable with `net.connman.iwd.Agent.Error.Canceled`.
  - A wrong password returns plain `net.connman.iwd.Failed`, with no specific error (src/station.c, src/network.c). iwd saves a network (`/var/lib/iwd/<ssid>.psk`, root-only) only after it connects.
- **Launcher today:**
  - `settings.rs` is a pure state machine. Its pages are Menu, Audio, Bluetooth, Device, Add and Mirroring. The Menu rows are a literal list (audio, bluetooth, mirroring), and `select()` matches the Menu by hardcoded row index.
  - Keys: the Slint `FocusScope` maps Up and Down, Return/Space to select, and Escape/Backspace to back. Typed characters (`event.text`) aren't forwarded anywhere, and Left/Right are ignored in Settings.
  - Gamepad evdev maps A to Activate and B to Back. X, Y and Start aren't mapped. Keyboard keys arrive only through Wayland.
- **Icons:** the embedded Material Icons font has `wifi` U+E63E, `wifi_off` U+E648, `signal_wifi_4_bar` U+E1D8, `signal_wifi_0_bar` U+F0B0, `lock` U+E897, `backspace` U+E14A, `keyboard_capslock` U+E318, `space_bar` U+E256, `visibility`/`visibility_off` U+E8F4/E8F5 and `keyboard_return` U+E31B (all checked in the font's cmap).

## How it gets made (my call)

**iwd replaces wpa_supplicant, but only for joining networks.** DHCP and routes stay with ifupdown and dhcpcd exactly as today: `iface wlp2s0 inet dhcp` with metric 600. So Ethernet, its metric and the DNS path don't change.

The launcher talks to iwd over D-Bus. It uses zbus like the Bluetooth worker, and an agent supplies the password, like the BlueZ pairing agent. iwd stores saved networks and autoconnects by itself.

I rejected two alternatives:
- **NetworkManager** would try to manage more of a box that's also a home server.
- **Driving wpa_supplicant's control socket ourselves** would mean building our own saved-network storage.

## Screens (as agreed)

**Settings menu:** rows Audio output, Bluetooth, **Wi-Fi**, Mirroring. The Wi-Fi detail shows the current network's name, "Not connected", or "Not available" (iwd missing or no Wi-Fi device). Menu selection matches on row keys, not indexes.

**Wi-Fi page:**
- **Title** "Wi-Fi".
- **Status line:**
  - "Connecting to X…" while a connect is in progress;
  - otherwise "Connected to X";
  - otherwise "Searching…" while a scan runs;
  - otherwise "Not connected";
  - "Wi-Fi isn't available" when there's no iwd or no device.
- **Rows** in iwd's order (connected first, then known, then the rest by signal):
  - **Icon:** a signal icon.
  - **Label:** the network name.
  - **Detail:** "Connected", "Connecting…", "Saved", "Not supported" (8021x and wep), or empty. A lock is shown for password networks.
- **Hidden networks:** networks without a name are left out.
- **Scanning:**
  - Entering the page powers the Wi-Fi device on if it's off, and starts a scan.
  - While the page is open, it scans again every 10 s. Busy errors are ignored.
  - Scanning stops being requested when the page is left.
- **Picking a row:**
  - A connected or saved network opens its **network page**.
  - A new open network connects immediately.
  - A new password network opens the **keyboard pop-up**.
  - "Not supported" shows the toast "X isn't supported".

**Network page:** the title is the network name. Rows are "Connect" (or "Disconnect" when connected), then "Forget". Forget removes the saved network (and disconnects it), then goes back.

**Results:**
- **Connect succeeds:** the box goes back to the Wi-Fi page, now showing "Connected to X".
- **Connect fails after typing a password:** the toast "Couldn't connect to X. Check the password." appears, and the pop-up reopens with the typed text kept. iwd can't tell a wrong password apart from other failures.
- **Other failures:** "Couldn't connect to X".
- **Disconnect:** at home, Disconnect leaves Wi-Fi off until a Connect, which is iwd's behaviour; Ethernet is unaffected.

**Keyboard pop-up:**
- **Layout:** it covers the bottom half of the screen, over the Wi-Fi page.
  - Title "Password for X", then the text field.
  - Below that, the key grid:
    - a row of digits 1–0;
    - letter rows q–p, a–l and z–m;
    - a bottom row with Shift, a symbols switch ("!#1"/"abc"), a wide Space, Delete, Show/Hide and **Connect**.
- **Characters:** the symbols layout replaces the letter rows. Between the two layouts, every printable ASCII character (32–126) can be typed.
- **Shift (my call):** it toggles until pressed again, and the keys show upper case while it's on.
- **Text field (my call):** dots by default; Show/Hide switches to plain text. There's a cursor at the end only, so editing is append and delete-last.
- **Controller and remote:**
  - Up, Down, Left and Right move the highlight. Left/Right stop at the row ends. Up/Down go to the key nearest horizontally.
  - A or OK presses the highlighted key.
  - B or Back closes the pop-up and discards the text.
  - **(my call)** gamepad X deletes a character, and Start (`BTN_START`) presses Connect. Linux numbers the face buttons differently per driver. xpad reports Xbox X as `BTN_X`, the same code as `BTN_NORTH`, while hid-playstation reports Square as `BTN_WEST`. So both codes (`BTN_NORTH`/`BTN_X` and `BTN_WEST`/`BTN_Y`) delete, which means the top face button deletes too.
- **Keyboard:**
  - Printable characters typed without Ctrl/Alt go straight into the field. Space types a space.
  - **(my call)** typing a character moves the highlight to Connect.
  - Enter presses the highlighted key. So after typing on a keyboard, Enter connects, and a remote or air mouse whose OK sends Enter still presses the highlighted key.
  - Backspace deletes; it doesn't mean Back here.
  - Escape closes.
  - The arrow keys still move the highlight.
- **Home** (the home key, or holding Guide) closes the pop-up and Settings, as anywhere in Settings.
- **Password length:**
  - Connect needs 8–63 characters. Otherwise the toast says "Wi-Fi passwords are 8 to 63 characters" and the pop-up stays. (iwd takes a password typed through its agent only as a passphrase, never as a 64-digit hex key.)
  - Open networks never show the pop-up.

## Architecture

| Unit | Responsibility |
|---|---|
| `wifi` (new, worker thread) | iwd over D-Bus (zbus blocking, one system-bus connection, lazily connected like `bluetooth`). Commands: `SetActive(bool)`, `Scan`, `Connect{path, passphrase: Option<String>}`, `Disconnect`, `Forget(path)`, `PowerOn`. Events: `State(WifiState)` (sent only on change, polled every 1 s while active through `GetManagedObjects` plus `GetOrderedNetworks`) and `OpFinished{op, path, result}`. |
| `wifi::agent` | Served at `/mushtv/wifi_agent` and registered with `AgentManager.RegisterAgent`. `RequestPassphrase` returns the passphrase held for the connect in flight (it's set just before `Connect()` and cleared right after). With no connect in flight, it returns `Canceled`. The passphrase is held only in memory and is never logged or put in an event. |
| `wifi::model` | Pure conversion of iwd properties into `WifiNetwork{path, name, security: Open/Psk/Unsupported, signal_dbm100, known, connected}` and `WifiState{available, powered, scanning, station_state, connected: Option<name>, networks}`. Also signal bars from dBm. |
| `osk` (new, pure) | The keyboard pop-up's model: layouts, Shift, symbols, focus movement, pressing a key, typed-character and Backspace handling, show/hide, and the Connect length check. It's a view model for the pop-up's Slint component. |
| `settings` | New pages Wifi and WifiNetwork{path}, plus the pop-up as a modal state on top of Wifi. New `Snapshot` fields from `WifiState`. New `Effect`s: `SetWifiActive`, `WifiScan`, `WifiConnect{path, passphrase}`, `WifiDisconnect`, `WifiForget`, `WifiPowerOn`. Menu `select()` by row key. New `Nav` variants (Left, Right, Delete, Submit, `Text(String)`, Backspace) are used only while the pop-up is open; elsewhere they do nothing. |
| `settings_ui` | Spawns the Wi-Fi worker, applies the new effects and renders the pop-up. |
| `ui/` | A keyboard pop-up component in the Settings look. `main.slint`'s FocusScope forwards Left/Right and typed text (`event.text`) to Rust while the pop-up is open, and Backspace/Escape/Enter as pop-up keys. |
| `input` | evdev `BTN_WEST` → `Action::Delete`, `BTN_START` → `Action::Submit`. They only matter while the pop-up is open. |

**Ethernet stays first:** dhcpcd's metrics (100 for Ethernet, 600 for Wi-Fi) already ensure it. The launcher doesn't touch routes.

## System changes and rollout (not run during the build)

Files in the repo, which `deploy.sh` installs on the next deploy (the user runs it at a time that suits them):

1. **D-Bus policy:** `deploy/dbus-mushtv-iwd.conf` → `/etc/dbus-1/system.d/mushtv-iwd.conf`. It lets user `tv` call `net.connman.iwd`. The bus reloads it on its own, so `tv` needs no new group and no new login.
2. **Keep the interface name:** `/etc/systemd/network/80-iwd.link` → `/dev/null` (a symlink), which masks Debian's renaming file.
3. **Keep the kernel's interface:** `deploy/iwd-main.conf` → `/etc/iwd/main.conf` with `[DriverQuirks] DefaultInterface=*`, so iwd uses `wlp2s0` instead of re-creating it. It also sets `[General] EnableNetworkConfiguration=false` explicitly, so DHCP stays with dhcpcd. These files do nothing until iwd is installed.

A one-time switch script, `deploy/wifi-switch-to-iwd.sh`. The user runs it with sudo, at a moment when nothing is streaming. It is never run by `deploy.sh`. It:

1. **Checks first:** it refuses to run unless Ethernet (`enp3s0`) has a default route. That way the box stays reachable over SSH and Tailscale if Wi-Fi drops during the switch.
2. **Backs up the config:** it copies `/etc/network/interfaces` to `interfaces.bak-<date>`.
3. **Installs iwd** and the two files above (if deploy hasn't yet).
4. **Moves the home network over:** it reads `wpa-ssid` and `wpa-psk` from the `wlp2s0` stanza and writes `/var/lib/iwd/<ssid>.psk` (mode 600), so the box still joins home Wi-Fi.
   - **Passphrase:** written as `Passphrase=`, escaped as iwd's format needs (`\\` for a backslash, `\s` for a leading space).
   - **64 hex digits:** written as `PreSharedKey=` only with `--hex-ok`, because iwd can't use a key with WPA3 routers.
   - **Hidden network** (`wpa-scan-ssid 1`): marked `[Settings] Hidden=true`.
   - **SSID characters:** SSIDs with characters outside iwd's plain filename rules use iwd's `=<hex>.psk` naming.
5. **Edits only the `wlp2s0` stanza:** `ifdown wlp2s0`, then remove its `wpa-*` lines (keeping `inet dhcp` and `metric 600`).
6. **Starts iwd:** `systemctl disable --now wpa_supplicant`, then `systemctl enable --now iwd`, then `ifup wlp2s0`.
7. **Reports the result:** it prints the result (`iwctl station wlp2s0 show`, `ip route`).
8. **Has a dry-run mode** that only prints what it would do.

It never touches `enp3s0`, Docker or Tailscale.

**Builder's call:** whether the switch script is idempotent or detects an already-switched box and exits.

## Errors

- **No Wi-Fi available** (iwd missing, D-Bus denied, no station): the menu says "Not available", the Wi-Fi page says "Wi-Fi isn't available", and actions show a toast. Details go to the journal.
- **iwd D-Bus errors** become a short toast. `Busy` and `InProgress` from `Scan` are ignored.
- **One Wi-Fi operation at a time**, like Bluetooth's `Busy`, with a 60 s timeout.
- **Agent asked with nothing in flight** (a saved network whose password changed): it cancels, and the toast says "Couldn't connect to X". The user can then Forget the network and join again.

## Testing

- **Unit tests:**
  - `wifi::model`: property parsing, security mapping, ordering, signal bars, nameless networks dropped.
  - `osk`: layouts cover printable ASCII, focus movement and edges, Shift, symbols, Delete, show/hide, typed text and Backspace, the length check.
  - `settings`: the Wi-Fi pages, the pop-up's open, close and keep-text-on-failure, effects on enter and leave (scan, active), menu-by-key, Home and Back from the pop-up.
  - The agent's "only during a connect" rule as a pure function.
  - The gamepad mapping: both `BTN_NORTH`/`BTN_X` and `BTN_WEST`/`BTN_Y` delete, and `BTN_START` submits.
  - The switch script's parsing of an `interfaces` stanza (passphrase vs hex PSK, odd SSIDs), run against fixture files.
- **Allowed checks that touch no system state:**
  - `scripts/test-deploy.sh`, which runs `deploy.sh` with `DESTDIR` into a temporary folder (no root, no systemctl), to check the three new deploy files;
  - the switch script's parsing, run against fixture files only.
- **No live tests against iwd** (it isn't installed). Add `#[ignore]` live tests like Bluetooth's, for later.
- **Headless screenshots, in a fake-data mode only:** an environment variable makes Settings use canned Bluetooth, audio and Wi-Fi data and touch no system service (no BlueZ agent, no adapter power, no PipeWire, no iwd). Shots: Settings menu, Wi-Fi page (connected, list with locks/Saved/Not supported), network page, keyboard pop-up (letters, Shift, symbols, Show on).
  - Note: today's headless runs that enter Bluetooth would power the real adapter and replace the TV's pairing agent, which could disturb the user's controller. That's why the fake mode is required now.
- **Manual on the TV, later, after the user runs the switch:** join a phone hotspot with a controller only, then with a keyboard; get a password wrong; Forget; check that Ethernet still holds the default route; reboot and check that home Wi-Fi rejoins and the name is still `wlp2s0`.

## Risks

- **DNS away from home:** Tailscale owns `/etc/resolv.conf`. Its upstream DNS servers were learned at home. At a hotel, names outside the tailnet may not resolve unless the tailnet has global nameservers set in the Tailscale admin console (Override local DNS). This affects Ethernet at a new place too. To be checked separately; not solved here.
- **Driver:** `rtw_8821ce` with iwd is untested on this box. The switch script's backup and Ethernet requirement are the way back: `systemctl mask --now iwd`, `ifdown wlp2s0`, restore `interfaces.bak-<date>`, `systemctl enable --now wpa_supplicant`, `ifup wlp2s0`. The README documents this.
  - **Why mask:** a plain disable doesn't stop the D-Bus activation that the TV's Settings would trigger.
  - **Why ifdown first:** otherwise ifupdown still counts `wlp2s0` as up and `ifup` does nothing.

## Left to the builder

- **Slint layout details** of the pop-up and the exact symbol set (within: all printable ASCII reachable).
- **Signal icon choice:** use glyphs present in the embedded font, or a 4-bar indicator drawn in Slint.
- **Fake-data mode:** its variable name and fixture format.
- **Switch script:** its structure and whether it's idempotent.
