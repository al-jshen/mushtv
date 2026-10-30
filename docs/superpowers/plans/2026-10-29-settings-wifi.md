# Settings: Wi-Fi with an on-screen keyboard — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Wi-Fi page in the launcher's Settings that scans, joins open and WPA-Personal networks through iwd, and takes passwords from an on-screen keyboard driven by a controller, a remote or a physical keyboard. A one-time script moves the box from wpa_supplicant to iwd.

**Architecture:**
- **Pure modules carry the logic and the tests:**
  - `wifi::model`: iwd properties → state;
  - `osk`: the keyboard;
  - the Wi-Fi pages, in `settings` (split into `settings/mod.rs` plus `settings/wifi.rs`).
- **I/O sits around them:**
  - a zbus worker thread for iwd, with a passphrase agent, mirroring `bluetooth/`;
  - Slint components;
  - deploy files;
  - a switch script.
- **Demo mode:** `MUSHTV_DEMO=1` swaps every system service for canned data, so screenshots touch nothing real.

**Tech stack:** the existing crate. That's Rust 2024, `slint` 1.18 (femtovg, winit-wayland) and `zbus` 5 (blocking), with `evdev` 0.13. Bash for the scripts. iwd 3.8 (Debian 13) on the box after the switch.

**Spec:** `docs/superpowers/specs/2026-10-29-settings-wifi-design.md`

## Global constraints

- **Nothing that changes the TV box during the build:**
  - no package installs;
  - no `deploy.sh` (only `scripts/test-deploy.sh`, which stages into a temp dir);
  - no restarts of services, cage, the launcher or interfaces;
  - no tests or headless runs that talk to the real iwd, BlueZ, wpa_supplicant or PipeWire.

  Headless screenshots run only with `MUSHTV_DEMO=1`.
- **The Wi-Fi password:**
  - It appears only in `osk` text, in the `Passphrase` newtype (whose `Debug` hides it), and in the agent's reply to iwd.
  - It never appears in a log line, an event `Debug`, a file the launcher writes, or the repo.
- **The switch script stays away from the rest of the system:** it never touches `enp3s0`, Docker or Tailscale, and is never called by `deploy.sh`.
- **Look:** the same as the other Settings pages. Rows are 1000×96 design units, with the glow on focus and the same fonts. Icons come from the embedded Material Icons font, using only glyphs it contains: `wifi` U+E63E, signal bars U+EBE4/U+EBD6/U+EBE1/U+E1D8 for 1–4 bars, `lock` U+E897, `keyboard_capslock` U+E318, `space_bar` U+E256, `backspace` U+E14A, `visibility`/`visibility_off` U+E8F4/U+E8F5.
- **Copy, verbatim:**
  - menu row "Wi-Fi", with detail "Not available" / "Not connected" / "Connecting…" / network name;
  - page title "Wi-Fi";
  - status lines "Wi-Fi isn't available", "Connecting to {name}…", "Connected to {name}", "Searching…", "Not connected";
  - row details "Connected", "Connecting…", "Not supported", "Saved";
  - network page rows "Connect", "Disconnect", "Forget";
  - keyboard title "Password for {name}";
  - toasts:
    - "{name} isn't supported"
    - "Wi-Fi passwords are 8 to 63 characters"
    - "Couldn't connect to {name}. Check the password."
    - "Couldn't connect to {name}"
    - "Couldn't disconnect from {name}"
    - "Couldn't forget {name}"
- **Timings:** poll iwd every 1 s while Settings is open, rescan every 10 s while the Wi-Fi page is in the stack, and expire a Wi-Fi operation after 60 s.
- **Commits:** the user commits (GPG signing), on master. Each task ends by handing the user the commit command, never by committing.

## Review focus

1. **A connect result arrives after the user left the Wi-Fi page or Settings** (Home pressed mid-connect). The keyboard must not pop back up on another page, and a failure still toasts. Test: `failure_after_leaving_wifi_does_not_reopen_keyboard` (Task 4).
2. **The network list reorders or shrinks under the focus** (rescans every 10 s, iwd ranks by signal). Focus follows the same network by key, with no index panic. Tests: `focus_follows_network_when_list_reorders` and `focus_clamped_when_networks_vanish` (Task 4).
3. **The password leaking** through `Debug` of effects or commands, or through logs. Tests: `passphrase_debug_hides_text` (Task 1), `effects_debug_never_shows_password` (Task 4). Task 5's worker logs only op and path.
4. **iwd missing or D-Bus denied** (every box before the switch). The menu says "Not available", the page says "Wi-Fi isn't available", there's no busy loop, and a repeated error is logged only when it changes. Tests: `no_device_means_unavailable` (Task 1), `unavailable_wifi_page_selects_nothing` (Task 4), `error_logged_only_when_it_changes` (Task 5).
5. **Physical-keyboard keys that mean something else outside the pop-up** (Space, Backspace, letters). Inside the pop-up, Space types a space and Backspace deletes. Outside, they select and go back as before, and letters do nothing. Tests: `keyboard_keys_do_nothing_without_keyboard` (Task 4) and the headless typing scenario (Task 6).

---

### Task 1: `wifi::model`

**Files:**
- Create: `launcher/src/wifi/mod.rs` (for now only `pub mod model;`)
- Create: `launcher/src/wifi/model.rs`
- Modify: `launcher/src/main.rs` (add `mod wifi;`)

**Interfaces:**
- Produces (all in `wifi::model`):
  - `pub enum Security { Open, Psk, Unsupported }` (Copy, Eq)
  - `pub enum StationState { #[default] Disconnected, Connecting, Connected, Disconnecting, Roaming }` (Copy, Eq, Default)
  - `pub enum WifiOp { Connect, Disconnect, Forget }` (Copy, Eq)
  - `pub struct WifiNetwork { pub path: String, pub name: String, pub security: Security, pub signal: i16, pub known: bool, pub connected: bool }` (Clone, Eq). `signal` is in dBm×100, as iwd gives it.
  - `pub struct WifiState { pub available: bool, pub powered: bool, pub scanning: bool, pub state: StationState, pub current: Option<String>, pub networks: Vec<WifiNetwork> }` (Clone, Eq, Default). `current` is the path of the network connected or being connected to; `networks` keeps iwd's order.
  - `pub struct Passphrase(String)` (Clone, Eq; a hand-written `Debug` prints `Passphrase(<hidden>)`), with `pub fn new(String) -> Self` and `pub fn expose(&self) -> &str`
  - `pub type Objects = HashMap<String, HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>>` (path → interface → properties)
  - `pub fn security_from_type(t: &str) -> Security`
  - `pub fn station_state(s: &str) -> StationState`
  - `pub fn signal_bars(signal: i16) -> u8`
  - `pub fn valid_passphrase(p: &str) -> bool`
  - `pub fn state_from(objects: &Objects, ordered: &[(String, i16)]) -> WifiState`
  - consts `IWD_DEVICE = "net.connman.iwd.Device"`, `IWD_STATION = "net.connman.iwd.Station"`, `IWD_NETWORK = "net.connman.iwd.Network"`

- [ ] **Step 1: Write the failing tests** in `model.rs` (`#[cfg(test)]`; build `OwnedValue`s the way `bluetooth/model.rs`'s tests do):
  - `security_maps_iwd_types`: "open" → Open, "psk" → Psk, "8021x"/"wep"/"x" → Unsupported.
  - `station_state_strings`: "connected" → Connected, "connecting" → Connecting, "disconnecting" → Disconnecting, "roaming" → Roaming, "disconnected"/"" → Disconnected.
  - `signal_bars_by_strength`: -5500 → 4, -6000 → 4, -6100 → 3, -6700 → 3, -7000 → 2, -7500 → 2, -7600 → 1, -9000 → 1.
  - `passphrase_length_rules`:
    - "1234567" → false and "12345678" → true;
    - 63×'a' → true;
    - 64×'a' → false (not hex), 64×'f' → true, 65×'f' → false;
    - "" → false.
  - `passphrase_debug_hides_text`: `format!("{:?}", Passphrase::new("hunter22".into()))` doesn't contain "hunter22".
  - `state_lists_networks_in_iwd_order`. Objects:
    - `/net/connman/iwd/0/4` has the Device (`Powered`=true) and the Station (`State`="connected", `Scanning`=false, `ConnectedNetwork`=`…/4/aa_psk`);
    - networks `…/4/aa_psk` (Name "Home", Type "psk", Connected true, KnownNetwork set), `…/4/bb_psk` ("Marriott-Guest", "psk"), `…/4/cc_open` ("Starbucks", "open").

    With ordered `[(aa,-5000),(bb,-6500),(cc,-7200)]`, the state is available, powered, Connected, with `current`=aa, names in that order, `known` only for Home, `connected` only for Home, signals kept, and securities Psk/Psk/Open.
  - `nameless_networks_are_dropped`: a network with Name "" listed in `ordered` doesn't appear.
  - `no_device_means_unavailable`: empty objects → `WifiState::default()` (available false).
  - `powered_off_device_is_available_but_empty`: a Device with Powered false and no Station → available true, powered false, networks empty, state Disconnected.
- [ ] **Step 2: Run them and check they fail.** `cd launcher && cargo test wifi::model`. Expected: compile errors for the missing items.
- [ ] **Step 3: Implement.**
  - **Station:** the first path, in sorted order, carrying `IWD_DEVICE`. The Station is the same path's `IWD_STATION`, if present.
  - **Current network:** `current` = `ConnectedNetwork` if present, else the first network with `Connected`=true.
  - **Known:** `known` = a `KnownNetwork` property is present.
  - **Ordering:** walk `ordered` only, skipping paths without `IWD_NETWORK` or with an empty Name.
  - **Passphrase rule:** `valid_passphrase` accepts 8–63 chars, or exactly 64 ASCII hex digits.
- [ ] **Step 4: Run them and check they pass.** `cargo test wifi::model`. Expected: PASS.
- [ ] **Step 5: Hand over the commit.** `git add launcher/src/wifi launcher/src/main.rs && git commit -m "Wi-Fi model: iwd state, signal bars, passphrase rules"`

### Task 2: `osk` (on-screen keyboard model)

**Files:** Create `launcher/src/osk.rs`. Modify `launcher/src/main.rs` (add `mod osk;`).

**Interfaces:**
- Produces:
  - `pub enum KeyKind { Char(char), Shift, Symbols, Space, Delete, ShowHide, Connect }` (Copy, Eq)
  - `pub struct KeyCap { pub kind: KeyKind, pub label: String, pub icon: bool, pub width: u8 }`. With `icon` true, `label` is a Material Icons glyph. `width` is in half-key units: a character key is 2.
  - `pub struct PlacedKey { pub row: usize, pub x: u8, pub cap: KeyCap, pub focused: bool }`. `x` is the left edge, in the same units, on a 22-unit-wide board with each row centred: `x0 = (22 − row width) / 2`.
  - `pub enum OskOutcome { None, Submit }`
  - `pub struct Osk` (Clone, Debug, Eq), with these methods:
    - `new() -> Self` (empty text, focus on 'q')
    - `text(&self) -> &str`
    - `shown_text(&self) -> String` (one '•' per character unless shown)
    - `shift(&self) -> bool`, `symbols(&self) -> bool`, `shown(&self) -> bool`
    - `placed(&self) -> Vec<PlacedKey>` (in row order)
    - `focus_index(&self) -> usize` (into `placed()`)
    - `move_by(&mut self, dx: i32, dy: i32)`
    - `press(&mut self) -> OskOutcome`
    - `press_index(&mut self, i: usize) -> OskOutcome` (mouse click: focus key `i`, then press)
    - `type_char(&mut self, c: char)`
    - `delete(&mut self)`
  - `pub const MAX_LEN: usize = 64`
- **Layouts:**
  - **Row 0, both layouts:** `1 2 3 4 5 6 7 8 9 0`.
  - **Letters:** rows 1–3 `q w e r t y u i o p` / `a s d f g h j k l` / `z x c v b n m`. Upper case while Shift is on.
  - **Symbols:** rows 1–3 `! @ # $ % ^ & * ( ) -` / `_ = + [ ] { } \ | ; :` / `' " , . < > / ? ` ~` (11, 11 and 10 keys).
  - **Row 4, both layouts:**
    - Shift (width 3, icon U+E318);
    - Symbols (width 3, text "!#1" in letters, "abc" in symbols);
    - Space (width 6, icon U+E256);
    - Delete (width 3, icon U+E14A);
    - ShowHide (width 3, icon U+E8F4 while hidden, U+E8F5 while shown);
    - Connect (width 4, text "Connect").

- [ ] **Step 1: Write the failing tests:**
  - `starts_on_q_with_empty_hidden_text`
  - `pressing_keys_types_them`: move to 'w' (right 1) and press → "w"; then Space → "w ".
  - `shift_types_upper_case_until_pressed_again`
  - `symbols_layout_types_punctuation`
  - `every_printable_ascii_is_typable`: for each c in ' '..='~', some combination of Shift/Symbols has a key that types c (Space for ' ').
  - `left_right_stop_at_row_ends`: from '1', Left stays; from '0', Right stays.
  - `up_down_go_to_the_nearest_key`, nearest by key centre:
    - q↓ → a;
    - p↓ → l;
    - z↓ → Symbols;
    - m↓ → ShowHide;
    - Space↑ → c;
    - '1'↑ stays;
    - Connect↓ stays.
  - `switching_layouts_clamps_focus`: on '-' (row 1, col 10 in symbols), switch back to letters → focus is in row 1 and ≤ col 9.
  - `delete_removes_the_last_character`, and does nothing on empty text.
  - `typing_on_a_keyboard_appends_and_focuses_connect`: `type_char('x')` → text "x", and the focused key is Connect.
  - `non_printable_typed_is_ignored`: '\u{f700}' (Slint's arrow private-use char), 'é' and '\n' leave the text unchanged.
  - `show_hide_toggles_dots`: text "abc" → `shown_text` "•••"; after ShowHide → "abc".
  - `connect_key_submits`: `press` on Connect → `OskOutcome::Submit`, and the text is unchanged.
  - `text_is_capped_at_64_characters`
  - `press_index_focuses_and_presses`
- [ ] **Step 2: Run them and check they fail.** `cargo test osk`.
- [ ] **Step 3: Implement.** Up/Down picks, in the target row, the key whose centre (`x + width/2`) is nearest the current key's centre; ties go left. Changing layout clamps the column to the row's length.
- [ ] **Step 4: Run them and check they pass.** `cargo test osk`.
- [ ] **Step 5: Hand over the commit.** `git add launcher/src/osk.rs launcher/src/main.rs && git commit -m "On-screen keyboard model"`

### Task 3: Input — Delete and Submit buttons

**Files:** Modify `launcher/src/input/logic.rs`, `launcher/src/input/mod.rs` and `launcher/src/main.rs` (exhaustive matches only).

**Interfaces:**
- Produces:
  - `RawEvent::DeleteButton(DevId)` and `RawEvent::StartButton(DevId)`;
  - `Action::Delete` and `Action::Submit`.
- **Mapping in `input/mod.rs`, gamepads only, key down:** `KeyCode::BTN_NORTH` (the same code as `BTN_X`) and `KeyCode::BTN_WEST` (the same code as `BTN_Y`) → `DeleteButton`; `KeyCode::BTN_START` → `StartButton`.
  - Why both: xpad reports Xbox X as `BTN_X`, and hid-playstation reports Square as `BTN_WEST`.
  - Put that reason in a one-line comment.
- **`main.rs`:** `on_action` ignores `Delete`/`Submit` everywhere for now. Task 6 routes them into Settings.

- [ ] **Step 1: Write the failing tests** in `logic.rs`:
  - `delete_and_start_buttons_act_when_navigation_allowed`: `DeleteButton` → `[Delete]` and `StartButton` → `[Submit]`.
  - `delete_and_start_ignored_while_app_runs_or_cooling_down`
- [ ] **Step 2: Run them and check they fail.** `cargo test input::logic`.
- [ ] **Step 3: Implement.** They're gated by `navigation_allowed`, like `South`.
- [ ] **Step 4: Run them and check they pass.** `cargo test`, the whole crate, which shows the new match arms compile.
- [ ] **Step 5: Hand over the commit.** `git add launcher/src/input launcher/src/main.rs && git commit -m "Input: Delete and Start buttons"`

### Task 4: Settings — Wi-Fi pages and the keyboard pop-up

**Files:**
- Move: `git mv launcher/src/settings.rs launcher/src/settings/mod.rs`, with no content change, as its own step. Check that `cargo test` still passes.
- Create: `launcher/src/settings/wifi.rs` for the Wi-Fi row, status and selection helpers, and their tests.
- Modify: `launcher/src/settings/mod.rs`.

**Interfaces:**
- Consumes:
  - from Task 1: `WifiState`, `WifiNetwork`, `Security`, `StationState`, `WifiOp`, `Passphrase`, `signal_bars` and `valid_passphrase`;
  - from Task 2: `Osk`, `PlacedKey` and `OskOutcome`.
- Produces (in `settings`):
  - `Snapshot.wifi: WifiState`
  - `Nav` gains `Left, Right, Delete, Submit, Char(char)` (stays `Copy`)
  - `Effect` gains:
    - `SetWifiActive(bool)`
    - `WifiPowerOn`
    - `WifiScanning(bool)`
    - `WifiConnect { path: String, passphrase: Option<Passphrase> }`
    - `WifiDisconnect(String)` (the network path, for the result)
    - `WifiForget(String)`
  - `Icon` gains `Wifi` and `WifiSignal(u8)` (1–4)
  - `RowView` gains `locked: bool` (false on every existing row)
  - `View` gains `keyboard: Option<KeyboardView>`, where `pub struct KeyboardView { pub title: String, pub text: String, pub keys: Vec<PlacedKey> }`
  - `pub const WIFI_BUSY_TIMEOUT: Duration` (60 s)
  - `Settings::wifi_finished(&mut self, op: WifiOp, path: &str, result: Result<(), String>, snap: &Snapshot) -> Vec<Effect>`
  - `Settings::keyboard_open(&self) -> bool`
  - `Settings::keyboard_click(&mut self, index: usize, snap: &Snapshot, now: Instant) -> Vec<Effect>`
- **Private state:**
  - `Page::Wifi` and `Page::WifiNetwork { path }`;
  - `wifi_busy: Option<WifiBusy { op, path, since, entry: Option<Osk> }>`, where `entry` is the typed password's keyboard, kept so it can reopen;
  - `popup: Option<Popup { path, name, osk: Osk }>`.
- **Behaviour (the spec's "Screens" section, plus):**
  - **Opening and closing Settings:**
    - `open()` → `[SetBtActive(true), SetWifiActive(true)]`.
    - `close()` also clears the pop-up, emits `WifiScanning(false)` if `Page::Wifi` is in the stack, and ends with `SetBtActive(false), SetWifiActive(false), Close`.
  - **Back:**
    - With the pop-up open, `back()` only closes the pop-up and emits no effect.
    - Popping `Page::Wifi` emits `WifiScanning(false)`.
  - **Menu:**
    - Rows, by key: `audio`, `bluetooth`, `wifi`, `mirroring`.
    - `select()` on the Menu matches the row **key**, not the index.
    - Entering Wi-Fi emits `WifiPowerOn` if it's available but not powered, then `WifiScanning(true)`.
  - **Rows:**
    - The Wi-Fi rows' key is the network path.
    - `checked` is set when the network is connected and the station is Connected.
    - `locked` is set when security is Psk.
    - The network page's row keys are `link` and `forget`.
  - **Pop-up keys:** with the pop-up open, `Up`/`Down`/`Left`/`Right` → `move_by`. `Select` → `press` (a `Submit` outcome submits). `Delete` → `delete`, `Submit` → submit, `Char(c)` → `type_char`, `Back` → close the pop-up, `Home` → `close()`. Without the pop-up, `Left`/`Right`/`Delete`/`Submit`/`Char` do nothing.
  - **Submitting a password:**
    - An invalid passphrase gets the toast and the pop-up stays open.
    - While another Wi-Fi operation runs, it's ignored.
    - Otherwise it sets busy (keeping the `Osk` as `entry`), closes the pop-up and emits `WifiConnect` with `Some(passphrase)`.
  - **`wifi_finished`:**
    - An error on Connect with an `entry`, while the top page is still `Wifi`, reopens the pop-up with that `Osk` and gives the "Check the password." toast.
    - Otherwise a failure gets the plain toast for its op.
    - Success does nothing.
  - **`on_data`:**
    - expires `wifi_busy` after `WIFI_BUSY_TIMEOUT`;
    - pops a `WifiNetwork` page whose path is no longer in `snap.wifi.networks`;
    - refocuses, as today.
- **Existing tests:** update the expected effects for open and close.

- [ ] **Step 1: Do the file move.** `git mv launcher/src/settings.rs launcher/src/settings/mod.rs`, then `cargo test`. Expected: PASS, with nothing else changed.
- [ ] **Step 2: Write the failing tests** in `settings/wifi.rs`.

  They use a fixture snapshot with networks in this order:
  - Home: psk, known, connected, -5000;
  - Marriott-Guest: psk, -6500;
  - Phone: psk, known, -7000;
  - Starbucks: open, -7200;
  - Corp: Unsupported, -8000.

  The state is Connected with `current` = Home.

  Tests:
  - `opening_activates_bluetooth_and_wifi`
  - `menu_has_wifi_row_with_current_network`: detail "Home". With `available` false it's "Not available"; while connecting, "Connecting…".
  - `menu_selects_by_key`: focus row 2 → title "Wi-Fi"; row 3 → "Mirroring".
  - `entering_wifi_powers_on_and_scans`: unpowered → `[WifiPowerOn, WifiScanning(true)]`; powered → `[WifiScanning(true)]`.
  - `leaving_wifi_stops_scanning`: Back → `[WifiScanning(false)]`.
  - `home_from_wifi_stops_scanning_and_deactivates`
  - `wifi_rows_show_signal_lock_and_status`:
    - labels in fixture order;
    - details `["Connected", "", "Saved", "", "Not supported"]`;
    - `checked` only on Home;
    - `locked` `[true, true, true, false, false]`;
    - icons `WifiSignal(4), WifiSignal(3), WifiSignal(2), WifiSignal(2), WifiSignal(1)`.
  - `wifi_status_line`: "Connected to Home"; scanning while disconnected → "Searching…"; disconnected → "Not connected"; unavailable → "Wi-Fi isn't available"; busy Connect on Marriott-Guest → "Connecting to Marriott-Guest…".
  - `unavailable_wifi_page_selects_nothing`
  - `selecting_a_new_password_network_opens_the_keyboard`: `view.keyboard` title "Password for Marriott-Guest", text "".
  - `selecting_an_open_network_connects_without_a_password`: `[WifiConnect { path: starbucks, passphrase: None }]`.
  - `selecting_unsupported_shows_toast`: "Corp isn't supported".
  - `selecting_saved_or_connected_opens_network_page`: Home → title "Home", rows "Disconnect"/"Forget"; Phone → "Connect"/"Forget".
  - `network_page_actions`: Connect → `WifiConnect { passphrase: None }`; Disconnect → `WifiDisconnect(path)`; Forget pops and emits `WifiForget(path)`.
  - `keyboard_submit_connects_with_the_typed_password`: `Char` ×8 for "hunter22", then `Submit` → `WifiConnect { passphrase: Some(Passphrase::new("hunter22")) }`, and `keyboard` is None.
  - `select_on_connect_key_submits_too`
  - `short_password_is_refused_with_toast`
  - `failed_connect_reopens_keyboard_with_text_kept`: "Couldn't connect to Marriott-Guest. Check the password." and keyboard text "••••••••".
  - `failed_open_connect_shows_plain_toast`: "Couldn't connect to Starbucks".
  - `failure_after_leaving_wifi_does_not_reopen_keyboard`: Home after submitting, then an error → toast only, and no keyboard on reopen.
  - `back_closes_only_the_keyboard`
  - `home_closes_keyboard_and_settings`
  - `keyboard_keys_do_nothing_without_keyboard`: `Left`, `Right`, `Delete`, `Submit`, `Char('a')` on the Wi-Fi page → no effects and no change to the view.
  - `focus_follows_network_when_list_reorders`
  - `focus_clamped_when_networks_vanish`
  - `network_page_pops_when_network_disappears`
  - `wifi_busy_times_out`: `on_data` at `since + 60 s` allows a new connect.
  - `effects_debug_never_shows_password`: `format!("{effects:?}")` after a submit doesn't contain "hunter22".
  - `keyboard_click_presses_that_key`
- [ ] **Step 3: Run them and check they fail.** `cargo test settings::wifi`.
- [ ] **Step 4: Implement**, in `settings/mod.rs` and `settings/wifi.rs`.
- [ ] **Step 5: Run them and check they pass.** `cargo test`, the whole crate, which includes the existing Settings tests.
- [ ] **Step 6: Hand over the commit.** `git add -A launcher/src/settings && git commit -m "Settings: Wi-Fi pages and password keyboard"`

### Task 5: iwd worker and agent

**Files:** Modify `launcher/src/wifi/mod.rs`. Create `launcher/src/wifi/agent.rs`.

**Interfaces:**
- Consumes: Task 1's model.
- Produces (in `wifi`):
  - `pub enum WifiCmd { SetActive(bool), SetScanning(bool), PowerOn, Connect { path: String, passphrase: Option<Passphrase> }, Disconnect(String), Forget(String) }` (Debug derives safely through `Passphrase`)
  - `pub enum WifiEvent { State(WifiState), OpFinished { op: WifiOp, path: String, result: Result<(), String> } }`
  - `#[derive(Clone)] pub struct WifiHandle`, with `send(&self, WifiCmd)`
  - `pub fn spawn(on_event: impl Fn(WifiEvent) + Send + Sync + 'static) -> WifiHandle`
  - in `wifi::agent`:
    - `pub const AGENT_PATH: &str = "/mushtv/wifi_agent"`
    - `pub fn passphrase_for(pending: Option<&(String, Passphrase)>, network: &str) -> Option<Passphrase>`
    - `pub struct Agent { pub pending: Arc<Mutex<Option<(String, Passphrase)>>> }`, serving `net.connman.iwd.Agent`
- **Worker, following `bluetooth/mod.rs`:** a named thread `wifi`, a lazy `Connection::system()`, and the agent served at `AGENT_PATH`. At connect it calls `RegisterAgent(o)` on `net.connman.iwd` `/net/connman/iwd` `net.connman.iwd.AgentManager`. On any D-Bus error it drops the bus, so it reconnects (and re-registers) next time.
  - **While active:** every 1 s it reads `ObjectManager.GetManagedObjects` on `net.connman.iwd` `/`, calls `Station.GetOrderedNetworks()` on the station path (signal is `i16`), and sends `State(state_from(…))` only when it changed.
  - **Scanning:** while scanning is wanted, `Station.Scan()` runs now and then every 10 s. `Busy`, `InProgress` and `Failed` errors from Scan are ignored.
  - **`SetActive(false)`** clears "scanning wanted".
  - **`PowerOn`:** `Device.Powered = true`.
  - **`Connect`:**
    1. Store `(path, passphrase)` in `pending` when there's a passphrase.
    2. Call `Network.Connect()` (blocking; the agent answers on zbus's own thread, as the BlueZ agent does).
    3. Clear `pending`.
    4. Send `OpFinished(Connect, …)`.
  - **`Disconnect(path)`:** `Station.Disconnect()`.
  - **`Forget(path)`:** read the `Network.KnownNetwork` property, then `KnownNetwork.Forget()`.
  - **Logging:** each op logs `wifi: {op:?} {path}: ok|{error}`, never the command. Errors from `state()` are logged only when they differ from the last one.
  - **Agent:**
    - `RequestPassphrase` returns `passphrase_for(...)`, else `net.connman.iwd.Agent.Error.Canceled`.
    - The private-key, username and user-password requests always return Canceled.
    - `Release` and `Cancel(reason)` log, without secrets.

- [ ] **Step 1: Write the failing tests:**
  - `passphrase_only_for_the_network_being_joined`, in `agent.rs`: None → None; another path → None; the same path → the passphrase.
  - `error_logged_only_when_it_changes`: a pure `fn should_log(last: &Option<String>, now: &str) -> bool` in `mod.rs`.
  - `#[ignore] live_reports_state`, like Bluetooth's live tests, for after the switch. It must not be run now.
- [ ] **Step 2: Run them and check they fail.** `cargo test wifi::`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them and check they pass.** `cargo test` (the ignored test is skipped) and `cargo clippy --all-targets`. Expected: PASS, no new warnings.
- [ ] **Step 5: Hand over the commit.** `git add launcher/src/wifi && git commit -m "Wi-Fi: iwd worker and passphrase agent"`

### Task 6: UI, wiring, demo mode and screenshots

**Files:**
- Create: `launcher/ui/keyboard.slint`, `launcher/src/demo.rs`, `launcher/tests/screens/wifi.sh`
- Modify: `launcher/ui/settings.slint`, `launcher/ui/main.slint`, `launcher/src/settings_ui.rs`, `launcher/src/main.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- **Slint:**
  - `SettingsRow` gains `locked: bool`. A locked row draws U+E897 just left of its detail.
  - **`keyboard.slint`:**
    - `export struct KeyCapData { label: string, icon: bool, row: int, x: int, width: int, focused: bool }`
    - `export component KeyboardPopup`, with `in` properties `title`, `text`, `keys: [KeyCapData]`, `u`, and `callback key-clicked(int)`.
    - The panel covers the bottom half, in the Settings look.
    - Keys are placed at `x` and `width` in half-key units, scaled to the panel. A focused key gets the row-style glow.
    - A full-window TouchArea behind the panel swallows clicks to the page beneath.
  - **`main.slint`:**
    - New properties: `settings-keyboard-open`, `settings-keyboard-title`, `settings-keyboard-text`, `settings-keyboard-keys`.
    - New callbacks: `settings-text(string)` and `settings-keyboard-click(int)`.
    - `settings-nav` codes become: 0 up, 1 down, 2 select, 3 back, 4 left, 5 right, 6 delete.
    - Key routing while the keyboard is open:
      - Up/Down → 0/1;
      - Return, "\n" or "\r" → 2;
      - Escape → 3;
      - Left/Right → 4/5;
      - Backspace → 6;
      - anything else with non-empty `event.text` and without Control or Alt → `settings-text(event.text)`. This includes Space, which types a space.
    - With the keyboard closed, routing stays as it is today.
- **`main.rs`:** in Settings, `Action::Left/Right/Delete/Submit` → `Nav::Left/Right/Delete/Submit`. Choice, Mirroring and the home screen ignore `Delete` and `Submit`.
- **`settings_ui.rs`:**
  - Spawns `wifi::spawn` next to Bluetooth and applies the six new effects.
  - `on_wifi(event)` updates `snap.wifi`, then calls `on_data` or `wifi_finished`.
  - Renders `locked` and the keyboard properties (`PlacedKey` → `KeyCapData`).
  - `on_settings_text` sends `Nav::Char(c)` for each char.
  - `on_settings_keyboard_click(i)` → `keyboard_click`.
  - Glyphs:
    - `Icon::Wifi` → U+E63E;
    - `WifiSignal(1..=4)` → U+EBE4, U+EBD6, U+EBE1, U+E1D8;
    - for the network page's Connect row, `Icon::Wifi`.
- **Demo mode:**
  - When `MUSHTV_DEMO` is set, `settings_ui::init` spawns neither worker and polls no audio, and `snap` starts as `demo::snapshot(mirroring_mode)`.
  - Effects meant for workers are dropped, with an info log ("demo: ignoring …" without the passphrase).
  - Toasts still show.
  - `demo::snapshot` holds two paired Bluetooth devices (a speaker and a controller), audio TV plus that speaker, and Task 4's five networks.
- **Scenario `tests/screens/wifi.sh`** (sourced by `scripts/headless.sh`):
  1. Navigate to the Settings tile and open it, then shoot `settings-menu`.
  2. Open Wi-Fi and shoot `wifi-list`.
  3. Open Home and shoot `wifi-network`.
  4. Back, pick Marriott-Guest and shoot `keyboard`.
  5. Type `wtype`'s "Hunter 22" (Space included) and shoot `keyboard-typed`. Expect 9 dots, with focus on Connect.
  6. Press Escape, then reopen Marriott-Guest. Toggle Shift on the on-screen keyboard and shoot `keyboard-shift`; then Symbols, and shoot `keyboard-symbols`; then Show/Hide, after typing a few keys, and shoot `keyboard-shown`.

  Add a `type_text` helper next to `key` if `headless.sh` lacks one (`wtype "<text>"`).

- [ ] **Step 1: Build.** `cargo build --release`. Expected: success. Then `cargo test`: PASS.
- [ ] **Step 2: Shoot.** `MUSHTV_DEMO=1 scripts/headless.sh launcher/launcher.toml /tmp/wifi-shots launcher/tests/screens/wifi.sh`. Expected: eight PNGs.
- [ ] **Step 3: Look at every screenshot and check it.**
  - The rows look like Bluetooth's.
  - Locks sit beside the password networks.
  - The check mark is on Home.
  - All keyboard keys are visible and inside the panel, the focus glow is on the right key, the dots count matches, and Shift and Symbols show the right characters.

  Fix and re-shoot until they're right.
- [ ] **Step 4: Hand over the commit.** `git add launcher/ui launcher/src launcher/tests/screens scripts/headless.sh && git commit -m "Settings: Wi-Fi screens, keyboard pop-up, demo mode"`

### Task 7: System files, the switch script, deploy and docs

**Files:**
- Create: `deploy/dbus-mushtv-iwd.conf`, `deploy/iwd-main.conf`, `deploy/wifi-switch-to-iwd.sh`, `scripts/test-wifi-switch.sh`, and fixtures in `scripts/fixtures/interfaces-*`
- Modify: `deploy.sh`, `scripts/test-deploy.sh`, `README.md`

**Interfaces:**
- **`deploy/dbus-mushtv-iwd.conf`:** a standard busconfig with one `<policy user="tv"><allow send_destination="net.connman.iwd"/></policy>`, and a comment saying why.
- **`deploy/iwd-main.conf`:**
  ```
  [General]
  EnableNetworkConfiguration=false

  [DriverQuirks]
  DefaultInterface=*
  ```
  With a comment: iwd otherwise deletes `wlp2s0` and creates `wlan0`, and dhcpcd (ifupdown) keeps doing DHCP with metric 600.
- **`deploy.sh` additions** (root part and DESTDIR alike):
  - `install_file 644 deploy/dbus-mushtv-iwd.conf /etc/dbus-1/system.d/mushtv-iwd.conf`;
  - `install_file 644 deploy/iwd-main.conf /etc/iwd/main.conf`;
  - ensure `/etc/systemd/network` exists and symlink `80-iwd.link` → `/dev/null`, replacing only a missing file or an existing symlink, never a regular file (warn instead).
- **`deploy/wifi-switch-to-iwd.sh [--dry-run]`**, run with sudo; the spec's eight steps, in order. Requirements:
  - Find the `iface wlp2s0` stanza: from that line to the next `iface`, `auto`, `allow-*`, `source`, `mapping` or EOF.
  - Read `wpa-ssid` and `wpa-psk` from it, stripping surrounding double quotes.
  - Install iwd with `apt-get install -y -o Dpkg::Options::=--force-confold iwd`, so the deployed `main.conf` is kept.
  - **The iwd file:**
    - **Name:** the SSID verbatim + `.psk` if it contains only alphanumerics, spaces, `_` and `-`; otherwise `=` + lower-case hex of the SSID + `.psk`.
    - **Content:** `[Security]` then `Passphrase=<psk>`, or `PreSharedKey=<psk>` for 64 hex digits.
    - Mode 600 in `/var/lib/iwd/`.
  - Comment out (prefix `# moved to iwd: `) every `wpa-*` line inside that stanza only.
  - In `--dry-run`, print each step and the planned file name, never the password, and change nothing.
  - **A parse-only mode for tests:** `--print-iwd-file <interfaces-file>` prints the file name on line 1 and the content after it. It needs no root.
- **`scripts/test-wifi-switch.sh`** checks, using the fixtures:
  - a passphrase → `Home Net.psk`, with `Passphrase=`;
  - 64 hex → `PreSharedKey=`;
  - SSID `café!` → `=636166c3a921.psk`;
  - a quoted SSID is unquoted;
  - no `wlp2s0` stanza → exits non-zero with a clear message;
  - other stanzas' `wpa-*` lines are ignored.
- **`scripts/test-deploy.sh`** gains checks:
  - D-Bus policy installed 644;
  - `main.conf` installed 644 and contains `DefaultInterface=*`;
  - `80-iwd.link` is a symlink to `/dev/null`;
  - a second run is still a no-op.
- **`README.md`:**
  - A "Wi-Fi" section next to Bluetooth: how to join, keyboard and controller keys, that Ethernet stays first, and the hotel sign-in limitation.
  - "Switching to iwd (once)": the command, that it needs Ethernet connected, and the rollback steps from the spec.
  - Checklist items for the manual tests in the spec.

- [ ] **Step 1: Write `scripts/test-wifi-switch.sh` and the fixtures.** Run it and check it fails (no script yet).
- [ ] **Step 2: Write the script.** Run `scripts/test-wifi-switch.sh`. Expected: all checks pass.
- [ ] **Step 3: Run `bash -n` and `--dry-run` on a fixture.** Run `bash deploy/wifi-switch-to-iwd.sh --print-iwd-file scripts/fixtures/interfaces-passphrase`. Expected: the name line and the content. Do **not** run the switch for real.
- [ ] **Step 4: Add the deploy test checks.** Run `scripts/test-deploy.sh` and check it fails, then add the files to `deploy.sh`. Run `scripts/test-deploy.sh`. Expected: "all deploy checks passed".
- [ ] **Step 5: Update the README.**
- [ ] **Step 6: Hand over the commit.** `git add deploy deploy.sh scripts README.md && git commit -m "Wi-Fi: iwd files, one-time switch script, docs"`

### Task 8: Verification and final review

- [ ] **Step 1: Run the full checks.** `cd launcher && cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`, then `scripts/test-deploy.sh` and `scripts/test-wifi-switch.sh`. Expected: everything passes.
- [ ] **Step 2: Re-shoot and review.** Re-run the demo screenshots and look at them.
- [ ] **Step 3: Final review.** Dispatch one review of the whole change on the most capable model, against the spec and this plan's Review Focus. Fix what it confirms.
- [ ] **Step 4: Report to the user** with the commit commands, how to deploy (`sudo ./deploy.sh --restart`), and the one-time switch (`sudo bash deploy/wifi-switch-to-iwd.sh --dry-run`, then without `--dry-run`). Both are for the user to run.
