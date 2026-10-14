# Settings (audio output + Bluetooth): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Built-in Settings in the launcher: Bluetooth pairing and management, plus audio output switching between the TV and Bluetooth speakers.

**Architecture:** Pure modules (`audio::parse`, `bluetooth::model`, `settings` state machine) carry the logic and the tests. Thin I/O wrappers sit around them: `pw-dump`/`wpctl` subprocesses, and a `zbus` blocking worker thread for BlueZ with a pairing agent. Slint renders a generic list-page view model.

**Tech stack:** existing crate, plus `zbus` 5 (pure Rust) and `serde_json`. Icons come from the Material Icons TTF (Apache-2.0).

**Spec:** `docs/superpowers/specs/2026-10-14-settings-audio-bluetooth-design.md`

**Execution:** Native (the user asked to build straight away).

## Global constraints

- No build-time `-dev` packages and no `pkg-config`.
- The TV session stays unprivileged; nothing new needs root at runtime.
- Polling only while Settings is open: Bluetooth once a second, audio once a second. Zero work when it's closed.
- Scanning is always stopped when leaving Add device or Settings.
- Connect-then-switch timeout: 15 s. Toasts use the existing 5 s toast.

## Review focus

1. **Settings closed by Home, or an app launched, while scanning or pairing.** Scanning must stop, and pairing results must not navigate a page that's gone. Test: `close_stops_scan`, `op_result_for_stale_page_is_ignored`.
2. **A speaker that never produces an output after connecting** (an audio profile the speaker doesn't support). The user gets the timeout toast and the choice stays unchanged. Test: `pending_switch_times_out`.
3. **BlueZ missing or the adapter gone mid-session.** Pages show "Not available" and there's no panic. Test: the view with `bt_available = false`; the worker reports unavailable on D-Bus errors.
4. **An agent request arriving when we didn't start a pairing** (a nearby device trying to pair). It's rejected. Test: `agent_rejects_unsolicited_confirmation`.
5. **The device list changing under the focus** (a device disappears while scanning). Focus is clamped, with no index panic. Test: `focus_clamped_when_rows_shrink`.

---

### Task 1: `audio` module
**Files:** `launcher/src/audio.rs`, `launcher/tests/fixtures/pw-dump.json` (or an inline const), `Cargo.toml` (`serde_json`).
**Produces:**
- `AudioState { outputs: Vec<Output>, configured: Option<String> }`
- `Output { id: u32, name: String, description: String, kind: OutputKind, bt_address: Option<String> }`
- `OutputKind { Tv, Bluetooth, Other }`
- `parse(&str) -> Result<AudioState>`, `bt_address_from_node_name(&str) -> Option<String>`, `read() -> Result<AudioState>`, `set_default(id: u32) -> Result<()>`

Tests:
- [ ] Sinks parse, and sources and streams are ignored.
- [ ] HDMI → Tv, bluez5 → Bluetooth (address from the prop), analog → Other.
- [ ] `configured` comes from the `default` metadata `default.configured.audio.sink` `{name}`; it's `None` when absent.
- [ ] The name → address mapping handles valid input and garbage.
- [ ] Invalid JSON → Err.

### Task 2: `bluetooth::model`
**Produces:**
- `DeviceKind { Audio, Gamepad, Keyboard, Mouse, Other }`, `kind_from_icon(Option<&str>) -> DeviceKind`
- `BtDevice { path: String, address: String, name: String, kind: DeviceKind, paired: bool, connected: bool, has_name: bool }`
- `device_from_props(path: &str, props: &HashMap<String, zvariant::OwnedValue>) -> Option<BtDevice>`

Tests:
- [ ] Icons: audio-card/headset/headphones/speakers → Audio; input-gaming → Gamepad; input-keyboard → Keyboard; input-mouse/tablet → Mouse; none/other → Other.
- [ ] Props: Alias is preferred over Name over Address; a missing address → None; booleans default to false.

### Task 3: `settings` state machine
**Produces:**
- `Snapshot { bt_available, powered, scanning, devices: Vec<BtDevice>, audio: Option<AudioState>, busy: Option<Busy>, passkey: Option<String> }`
- `Nav { Up, Down, Select, Back, Home }`
- `Effect { Close, SetBtActive(bool), StartScan, StopScan, Pair(path), Connect(path), Disconnect(path), Forget(path), SetOutput(u32), Toast(String) }`
- `View { title, status, rows: Vec<RowView{icon, label, detail, checked, dim}>, focused }`
- `Settings::new()`, `open()`, `is_open()`, `nav(Nav, &Snapshot, Instant) -> Vec<Effect>`, `on_data(&Snapshot, Instant) -> Vec<Effect>`, `op_finished(&BtOp, address, Result, &Snapshot) -> Vec<Effect>`, `view(&Snapshot) -> View`

Tests (first), as listed in Review focus plus:
- [ ] Menu → Audio → Back → menu → Back → Close.
- [ ] Home from a deep page → Close (and StopScan if scanning).
- [ ] Up/Down are clamped.
- [ ] Audio rows: TV checked with no configured default; a speaker checked when configured is its `bluez_output`.
- [ ] Selecting a connected speaker → SetOutput(id).
- [ ] Selecting a disconnected speaker → Connect + pending; a later snapshot with its sink → SetOutput.
- [ ] Bluetooth rows: paired devices plus "Add device"; selecting a device opens its page; Connect/Disconnect rows follow state; Forget → Forget effect + Back.
- [ ] Entering Add → StartScan; leaving → StopScan; rows show unpaired named devices; Select → Pair + busy; success → back to Bluetooth.
- [ ] Opening Settings → SetBtActive(true); closing → SetBtActive(false).

### Task 4: Bluetooth worker + agent (`bluetooth/mod.rs`, `bluetooth/agent.rs`)
- [ ] `zbus` blocking connection; poll `GetManagedObjects` while active; compute availability and powered from `Adapter1`.
- [ ] Commands as in the spec; `OpFinished` events; on D-Bus errors report `available = false` and keep running.
- [ ] Agent: pure decision function `agent_allows(kind, pairing_in_progress) -> bool`, tested (`agent_rejects_unsolicited_confirmation`); an interface wrapper registers it.
- [ ] Smoke test (`#[ignore]`, run manually): spawn against the real bus and get a `Devices` event with `available = true`.

### Task 5: Input Back
- [ ] logic: `RawEvent::BackKey`/`East` → `Action::Back`, gated like South. Tests.
- [ ] evdev: `BTN_EAST` on gamepads; `KEY_BACK`/`KEY_EXIT` on keyboard-like devices (`back_source`).

### Task 6: UI + wiring
- [ ] Material Icons TTF + licence into `assets/fonts/`.
- [ ] `ui/settings.slint` list page; gear tile; `MainWindow` props `settings-open`, `settings-title`, `settings-status`, `settings-rows`, `settings-focused`, callback `settings-nav(int)`; FocusScope routes keys while Settings is open.
- [ ] main: Settings state on the UI thread; a 1 s timer polls audio while open; Bluetooth events update the snapshot and re-render; effects are executed; Home/Back/Up/Down routing; the gear tile index opens Settings.
- [ ] Headless screenshots of each page (Bluetooth unavailable here is fine for some).

### Task 7: WirePlumber rule + deploy
- [ ] Add the `monitor.bluez.rules` priority rule; check that tv's WirePlumber-equivalent config parses (mushy's WP restart with the fragment, then revert); `scripts/test-deploy.sh` stays green.
- [ ] README: a Settings section, and the Wi-Fi/Bluetooth coexistence note.

### Task 8: Verification + review
- [ ] `cargo test`, clippy, fmt, release build, deploy tests; a fresh reviewer on the most capable model; fix Critical/Important findings test-first.
