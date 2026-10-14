# Settings: audio output and Bluetooth (design)

Date: 2026-10-14. This extends `2026-10-13-tv-launcher-design.md`. Decisions were agreed in conversation; the user approved the screens and asked for implementation.

## Goal

From the couch, using the remote or a gamepad:

- pair any Bluetooth device (speakers, controllers, keyboards), connect, disconnect and forget it;
- choose where audio plays: the TV (HDMI) or a paired Bluetooth speaker or headphones.

The audio choice is remembered. A chosen speaker gets the audio whenever it's connected; when it isn't, audio falls back to the TV. If the TV is chosen, a speaker turning on doesn't take over. Switching moves playing audio immediately, without restarting the app.

## Verified facts

- **Radio and drivers.** The Realtek RTL8821CE Bluetooth radio (USB `0bda:c821`, `hci0`, `btusb` driver) has its firmware installed. `bluez` 5.82 and `libspa-0.2-bluetooth` 1.4.2 are installed and the service is active.
- **Permissions.** The BlueZ D-Bus policy allows any local user to call `org.bluez`, so `tv` needs no extra groups.
- **Seat.** WirePlumber 0.5.8 enables its Bluetooth monitor only for the user on an active seat (`monitor.bluez.seat-monitoring` via logind). That's `tv`, so `mushy`'s PipeWire won't compete for speakers.
- **Default output.** WirePlumber keeps `default.configured.audio.sink` (a node name) in the `default` metadata, and stores it, plus a history of earlier choices, when it's set via `wpctl set-default`. If the chosen node is missing, WirePlumber prefers the most recently chosen node that's still available (scored `priority + 20001 − position`) before plain priority. It switches back when the chosen node returns. `linking.follow-default-target = true`, so streams move with the default.
  - **Consequence:** choosing a speaker sets the TV first and then the speaker, which keeps the TV second in that history. Without this, an older speaker in the history would win the fallback over the TV.
- **Priorities.** The analog sink has priority 1009, and BlueZ sinks default higher. Without a stored choice, a new speaker would take over. A rule gives `bluez_output.*` nodes priority 100, so the TV wins until the user chooses.
- **Node names.** Bluetooth sinks are named `bluez_output.<MAC with _>.<n>` and carry `api.bluez5.address`. The HDMI sink is an `alsa_output…hdmi…` node.
- **Wi-Fi coexistence.** The box is on Wi-Fi, which uses the same RTL8821CE combo chip as Bluetooth, so Bluetooth audio may stutter if the Wi-Fi is on 2.4 GHz. This is documented, not solved in code.

## Screens

- A built-in **Settings** tile (gear icon) is always last on the home row and isn't part of `launcher.toml`. Opening it replaces the home screen in the same window.
- **Settings menu:** "Audio output" (detail: the current choice) and "Bluetooth" (detail: "N devices", "Off" or "Not available").
- **Audio output:** "TV" plus every paired audio device. Each device shows "Connected", "Not connected" or "Connecting…", and the current choice has a check mark.
  - Picking TV, or a connected speaker, switches right away.
  - Picking a disconnected speaker connects it, then switches once its output appears. If that takes more than 15 s, a toast says "Couldn't connect <name>" and the choice is unchanged.
- **Bluetooth:** the paired devices (name, type icon, status), then "Add device". Picking a device opens its page.
- **Device page:** title is the device name; rows are "Connect" or "Disconnect", then "Forget". Forget removes the device and goes back.
- **Add device:** scans for as long as it's open, showing nearby unpaired devices that have a name, with the status line "Searching…". Picking one pairs, trusts and connects it ("Pairing…"), then goes back to Bluetooth. On failure a toast says "Couldn't pair <name>". If the device shows a passkey (keyboards), the status line shows "Type <code> on <name>".
- **Keys:**
  - Up/Down move, OK/Enter/A selects.
  - Back (Escape, Backspace, gamepad B, remote Back/Exit) goes up a level; from the menu it returns to the tiles.
  - Home (the configured key, or holding Guide) returns to the tiles from anywhere in Settings.
- **Look:** the same background, font and clock as the home screen. A page title, an optional status line, then a centered column of rows 1000×96 design units with an icon, label and right-aligned detail. The focused row gets the white border and glow. The list scrolls to keep the focused row visible. Icons come from the Material Icons font (Apache-2.0, Debian `fonts-material-design-icons-iconfont`), embedded by codepoint.

## Architecture

New modules in the launcher crate:

| Module | Responsibility | Interface |
|---|---|---|
| `audio` | Parse `pw-dump` JSON into outputs plus the configured default; set the default | `parse(json) -> AudioState`, `read() -> Result<AudioState>`, `set_default(id) -> Result<()>` |
| `bluetooth` | BlueZ over D-Bus (`zbus`, blocking API) on a worker thread; pairing agent | `Bluetooth::spawn(on_event) -> BluetoothHandle`, `send(BtCmd)`; events `BtEvent` |
| `bluetooth::model` | Pure conversion of BlueZ properties into `BtDevice`; device kind from `Icon` | `device_from_props`, `kind_from_icon` |
| `settings` | Pure page stack, focus and view model; turns keys and data into effects | `Settings::nav(Nav, &Snapshot, now) -> Vec<Effect>`, `view(&Snapshot) -> View`, `on_data(&Snapshot, now) -> Vec<Effect>` |
| `ui/settings.slint` | Draws a `View` (title, status, rows, focus) | properties only |

- **Bluetooth worker.**
  - It holds one system-bus connection.
  - While active (Settings open), it polls `GetManagedObjects` once a second and emits `Devices` when anything changed.
  - Commands run one at a time: `SetActive`, `StartScan`/`StopScan` (`Adapter1.StartDiscovery`/`StopDiscovery`), `Pair` (`Device1.Pair`, then `Trusted = true`, then `Connect`), `Connect`, `Disconnect`, `Forget` (`Adapter1.RemoveDevice`). Each sends `OpFinished { op, address, result }`.
  - Opening the Bluetooth pages sets the adapter's `Powered = true`.
- **Pairing agent.**
  - Registered at `/mushtv/agent` with capability `KeyboardDisplay` and requested as the default agent.
  - `DisplayPasskey` and `DisplayPinCode` emit an event that shows the code.
  - `RequestConfirmation` and `RequestAuthorization` are accepted only while a pairing started from the TV is in progress; otherwise they're rejected (`org.bluez.Error.Rejected`).
  - `RequestPinCode` returns "0000", for legacy speakers.
  - `AuthorizeService` is accepted only during that pairing window.
  - The adapter is never made discoverable.
- **Audio.**
  - `pw-dump` runs once a second while the Audio or Settings-menu pages are visible.
  - Choosing runs `wpctl set-default <id>`, which WirePlumber stores.
  - The configured default's node name maps to a device: `bluez_output.AA_BB…` → `AA:BB:…`, and alsa HDMI → TV.
  - No configured default counts as TV.
- **Input.**
  - New `Action::Back`: gamepad `BTN_EAST`, plus remote `KEY_BACK`/`KEY_EXIT` read from evdev (no usable keysyms, like OK/Select). Escape and Backspace come through Wayland.
  - Gamepad Up/Down already exist.
  - Navigation from evdev is gated as before.
  - `Home` while no app is running closes Settings.
- **WirePlumber rule.** `deploy/wireplumber-hdmi.conf` gains `monitor.bluez.rules` setting `priority.session`/`priority.driver = 100` on `bluez_output.*`. Because the file changes, deploy restarts tv's WirePlumber, which also loads the newly installed Bluetooth plugin.

## Errors

- **No adapter or BlueZ down:** Bluetooth shows "Not available", Audio lists only TV, and actions show a toast.
- **D-Bus call fails:** a toast with a short reason; the details go to the journal.
- **`pw-dump`/`wpctl` fails:** the Audio page shows TV only; a failed switch shows the toast "Couldn't switch audio".
- **Timeouts:** pairing is bounded by BlueZ's own timeout; connect-then-switch waits 15 s.
- **Scanning** is always stopped when leaving Add device or Settings.

## Testing

- **Unit tests:**
  - `audio::parse` against a fixture modelled on this machine's `pw-dump` (HDMI sink, a Bluetooth sink, default metadata);
  - the `bluez_output` name → address mapping;
  - `device_from_props` and `kind_from_icon`;
  - the `settings` state machine (navigation, back and home, the effects each page emits, pending connect-then-switch success and timeout, scan start/stop on enter/leave, the checked row);
  - input logic for Back.
- **Smoke test against the real BlueZ** (read-only): the worker lists the adapter and devices.
- **Headless screenshots** of each Settings page.
- **Manual on the TV:** pair a speaker and a controller, switch audio both ways, turn the speaker off and on, and check a keyboard passkey.
