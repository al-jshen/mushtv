# mushtv launcher: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `mushtv-launcher`, a Slint/Rust 10-foot app launcher that runs under cage as `tv`, plus `deploy.sh` to install it and the boot-to-kiosk setup.

**Architecture:** One binary with focused modules: `config`, `runner`, `input` (device readers plus a pure `logic` state machine), and a Slint UI. `main` wires them together. Apps run as child processes in their own process group; cage shows the app on top and hands the screen back when it exits.

**Tech stack:** Rust 1.99, Slint 1.18 (winit-wayland, femtovg), evdev 0.13, serde/toml, nix, tracing + tracing-journald, chrono.

**Spec:** `docs/superpowers/specs/2026-10-13-tv-launcher-design.md`

**Execution:** Native (the user asked to build straight away).

## Global constraints

- No build-time system `-dev` packages and no `pkg-config`. Every dependency must be pure Rust or dlopen its libraries.
- Nothing in the TV session needs root. `deploy.sh` is the only thing that does.
- The default config path is `/etc/mushtv/launcher.toml`; the source of truth is `launcher/launcher.toml`.
- Journal identifiers: `mushtv-launcher` for the launcher, `mushtv-<tile-id>` for apps (stdout/stderr via `systemd-cat -t`).
- Timings: Guide hold default 1000 ms (allowed 300–5000); Home debounce 1000 ms; return cooldown 500 ms; repeat 400 ms then every 150 ms; stop escalation 3 s + 3 s; "unexpected exit" window 10 s.
- The UI is designed at 1920×1080 and scaled by `height / 1080`.

## Review focus

1. **An app that never shows a window, or hangs while opening.** The "Opening…" overlay must not trap the user: Home must stop the app and return. Test: Home is accepted in the `Opening` state (task 3 logic gating, task 5 runner `stop()` while starting).
2. **A command whose direct process exits at once but leaves children running** (single-instance hand-off, `sh -c 'app &'`). The launcher must treat the app as running until the whole process group is gone, or Home can't reach it. Test: `sh -c 'sleep 2 & exit 0'` reports exit only after about 2 s (task 5).
3. **A controller connected or reconnected after start** (Bluetooth wake). It must be picked up. Test: `diff_devices` adds new nodes and drops vanished ones (task 4).
4. **A broken tile** (binary missing, Flatpak uninstalled). The user gets a message, not a hang. Tests: a missing binary gives `SpawnError`; `sh -c 'exit 3'` gives `Exited { code: Some(3), quick: true }` (task 5).
5. **Repeated or held Home during Stopping.** No double stop, no panic. Test: a second `stop()` is a no-op while stopping, and the logic debounce suppresses repeats (tasks 3 and 5).

---

## File structure

```
launcher/
  Cargo.toml            crate + deps
  build.rs              slint-build compile of ui/main.slint
  launcher.toml         the real config (source of truth)
  ui/main.slint         window, background, clock, row, overlays
  ui/tile.slint         one tile component
  assets/fonts/         Inter (OFL) + LICENSE
  src/main.rs           wiring, CLI, logging, signals, clock timer
  src/config.rs         Config/Tile structs, load, validate, icon resolution
  src/runner.rs         spawn/stop/exit tracking
  src/input/mod.rs      device discovery, readers, hotplug, InputAction
  src/input/logic.rs    pure state machine
  tests/                (unit tests live in-module under #[cfg(test)])
deploy.sh               installer (DESTDIR-aware for dry runs)
deploy/autologin.conf   getty override
deploy/bash_profile     tv login script
scripts/devtools.sh     fetch grim/wtype .debs into launcher/.devtools (no root)
scripts/headless.sh     run launcher under headless cage, screenshot
```

---

### Task 1: Scaffold and prove Slint renders under headless cage

**Files:** create `launcher/Cargo.toml`, `launcher/build.rs`, `launcher/ui/main.slint` (a placeholder window), `launcher/src/main.rs` (minimal), `scripts/devtools.sh`, `scripts/headless.sh`, and `.gitignore` (`launcher/target`, `launcher/.devtools`).

**Produces:** a buildable crate; `scripts/headless.sh <config> <out.png> [seconds] [wtype-script]`.

- [ ] Cargo.toml: `slint = { version = "1.18", default-features = false, features = ["std", "compat-1-2", "backend-winit-wayland", "renderer-femtovg"] }`, `slint-build = "1.18"`. Add the other dependencies now so task 1 proves they all build without pkg-config.
- [ ] `cargo build --release`. Expected: success with no pkg-config errors. If a dependency needs pkg-config, replace it, or use its dlopen feature.
- [ ] `scripts/devtools.sh`: `apt download grim wtype` into a fresh `launcher/.devtools/debs/`, then `dpkg -x` into `launcher/.devtools/root/`.
- [ ] `scripts/headless.sh`: `WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 cage -- sh -c '<launcher> --config X & sleep N; grim out.png; kill launcher'`. Set the headless output to 1920×1080 if cage allows; otherwise accept its default.
- [ ] Run it and look at the screenshot. Expected: the placeholder text is visible.

### Task 2: Config

**Files:** `launcher/src/config.rs`, `launcher/launcher.toml`.

**Produces:**
```rust
pub enum ClockStyle { H24, H12 }
pub struct Config { pub clock: ClockStyle, pub home_keys: Vec<evdev::KeyCode>, pub guide_hold: Duration, pub tiles: Vec<Tile> }
pub struct Tile { pub id: String, pub name: String, pub command: Vec<String>, pub stop_command: Option<Vec<String>>, pub icon: Option<PathBuf>, pub color: Option<String> }
pub fn load(path: &Path, icon_roots: &IconRoots) -> anyhow::Result<Config>
pub fn parse(text: &str, icon_roots: &IconRoots) -> anyhow::Result<Config>
pub struct IconRoots { pub assets: PathBuf, pub flatpak_icons: Vec<PathBuf> }
pub fn flatpak_app_id(command: &[String]) -> Option<&str>
pub fn parse_color(s: &str) -> anyhow::Result<(u8, u8, u8)>
```

Tests (write first):
- [ ] The minimal valid config parses, and defaults apply (24h, `KEY_HOMEPAGE`, 1000 ms).
- [ ] Errors, each with a message naming the field: no tiles; empty command; duplicate id; bad id characters; unknown key name; `guide_hold_ms` 100 and 9000; bad color.
- [ ] `flatpak_app_id(["flatpak","run","--branch=stable","a.b.C","--x"]) == Some("a.b.C")`; a non-flatpak command gives `None`.
- [ ] Icon resolution against a temp dir: explicit relative icon → assets dir; Flatpak with `scalable/apps/<id>.svg` and `256x256/apps/<id>.png` → SVG; PNG-only → the largest size; none → `None`.

### Task 3: Input logic state machine

**Files:** `launcher/src/input/logic.rs`.

**Produces:**
```rust
pub enum RawEvent { HomeKey, GuideDown(DevId), GuideUp(DevId), Dir { dev: DevId, axis: Axis, value: i8 } /* -1,0,1 after deadzone */, South(DevId), DeviceGone(DevId) }
pub enum Axis { X, Y }
pub enum Action { Left, Right, Up, Down, Activate, Home }
pub struct Logic { /* config + state */ }
impl Logic {
  pub fn new(guide_hold: Duration) -> Self;
  pub fn set_app_active(&mut self, active: bool, now: Instant); // true while Opening/Running/Stopping; false → starts the 500 ms cooldown
  pub fn on_event(&mut self, ev: RawEvent, now: Instant) -> Vec<Action>;
  pub fn on_tick(&mut self, now: Instant) -> Vec<Action>;   // fires holds and repeats
  pub fn next_deadline(&self) -> Option<Instant>;
}
```

Tests (write first, all with synthetic `Instant`s):
- [ ] Guide held ≥ hold → exactly one `Home` from `on_tick`; released early → nothing; held for a long time → still one.
- [ ] `HomeKey` → `Home`; a second within 1000 ms → nothing; after 1000 ms → `Home`.
- [ ] Home is accepted while the app is active (including while Opening) and while idle.
- [ ] App active: `Dir` and `South` produce nothing.
- [ ] Idle: `Dir X -1` → `Left` immediately; held → repeats at +400 ms, then every 150 ms; released (value 0) → stops.
- [ ] Idle: `South` → `Activate`.
- [ ] Cooldown: after `set_app_active(false, t)`, `South` at t+300 ms → nothing; at t+600 ms → `Activate`.
- [ ] `DeviceGone` clears that device's hold and repeat state (no ghost Home or repeat).

### Task 4: Input devices

**Files:** `launcher/src/input/mod.rs`.

**Produces:**
```rust
pub fn spawn(home_keys: Vec<evdev::KeyCode>, guide_hold: Duration, app_active: Arc<AtomicBool>, on_action: impl Fn(Action) + Send + 'static)
pub(crate) fn classify(dev: &evdev::Device, home_keys: &[KeyCode]) -> DevKind  // { gamepad, home_source } flags
pub(crate) fn diff_devices(known: &HashSet<PathBuf>, present: &HashSet<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>)
pub(crate) fn axis_to_dir(value: i32, info: AbsInfo) -> i8  // 50 % deadzone; hat values pass through
```

- [ ] Tests: `diff_devices` adds and removes correctly; `axis_to_dir` covers stick centre, edges and deadzone boundaries, plus hat −1/0/1.
- [ ] Implement:
  - startup scan of `/dev/input/event*` plus an inotify thread (`IN_CREATE | IN_ATTRIB | IN_DELETE`; retry opening on ATTRIB because udev sets permissions after create);
  - one blocking reader thread per useful device;
  - a logic thread with a channel and `recv_timeout(next_deadline)`. It mirrors `app_active` into `Logic::set_app_active` whenever the flag changes.
- [ ] Manual check: `cargo run -- --print-input` (a debug flag) lists the devices it found. No gamepad is attached now, so this mainly proves permissions and the scan don't crash.

### Task 5: Runner

**Files:** `launcher/src/runner.rs`.

**Produces:**
```rust
pub enum RunEvent { Exited { tile: usize, code: Option<i32>, quick: bool }, SpawnError { tile: usize, error: String } }
pub struct Runner { /* current: Option<Running> */ }
impl Runner {
  pub fn new(on_event: impl Fn(RunEvent) + Send + Sync + 'static, journal: bool) -> Self;
  pub fn launch(&self, tile_index: usize, tile: &Tile) -> bool;   // false if something is already running
  pub fn stop(&self);                                              // idempotent; async escalation thread
  pub fn stop_blocking(&self, timeout: Duration);                  // used at shutdown
  pub fn is_active(&self) -> bool;
}
```

Implementation:
- `Command` with `process_group(0)` and `pre_exec(prctl(PR_SET_PDEATHSIG, SIGTERM))`. When `journal` is set, prefix with `systemd-cat -t mushtv-<id> --`.
- A waiter thread does `child.wait()`, then polls `kill(-pgid, 0)` every 200 ms until ESRCH, then emits `Exited`.
- `quick` = non-zero exit within 10 s.

Tests (real processes; `journal = false`):
- [ ] `sh -c 'exit 0'` → `Exited { code: Some(0), quick: false }`.
- [ ] `sh -c 'exit 3'` → `Exited { code: Some(3), quick: true }`.
- [ ] `/nonexistent/bin` → `SpawnError`.
- [ ] `sh -c 'sleep 2 & exit 0'` → `Exited` arrives after ≥ 1.8 s.
- [ ] Stop: `sleep 30` → stopped by SIGTERM in under 1 s.
- [ ] Escalation: `sh -c 'trap "" TERM; sleep 30 & wait'` with a grandchild that also ignores TERM → gone by SIGKILL after about 3 s, and no process is left in the group.
- [ ] `stop_command` that kills the app (`["sh","-c","kill $(cat pidfile)"]`) → exits without reaching SIGTERM.
- [ ] `launch` while active → `false`; calling `stop()` twice is harmless.

### Task 6: UI

**Files:** `launcher/ui/main.slint`, `launcher/ui/tile.slint`, `launcher/assets/fonts/*`.

**Produces** (Slint `export component MainWindow`):
```
in property <[TileData]> tiles;      // struct TileData { name: string, icon: image, has-icon: bool, color: color }
in-out property <int> focused;
in property <string> clock-time; in property <string> clock-date;
in property <string> busy-text;      // "" = hidden
in property <string> toast-text;     // "" = hidden
in property <string> error-title; in property <string> error-details;  // "" = hidden
in property <bool> input-locked;     // keyboard/pointer ignored when true
callback activate(int);
public function move-focus(delta: int);
public function activate-focused();
```
- [ ] Fetch Inter with `apt download fonts-inter` into a fresh directory, extract, and copy the regular and semibold weights (or the variable font) plus the licence into `assets/fonts/`. Embed via `import "../assets/fonts/…";` and `default-font-family`.
- [ ] Build the layout per the spec's visual design section (gradient, clock, row with scroll, focus scale/glow/border, name label, opening overlay with spinner, toast, error screen). Use a FocusScope for Left/Right/Enter/Return/keypad Enter; TouchArea for hover and click.
- [ ] Screenshot checks via `scripts/headless.sh`: three tiles with real icons; focus on the second; the opening overlay; a toast; the error screen. Look at each PNG.

### Task 7: Wire up main

**Files:** `launcher/src/main.rs`.

- [ ] CLI: `--config <path>` (default `/etc/mushtv/launcher.toml`), `--check-config <path>` (print OK or the error and exit 0/1), `--print-input`.
- [ ] Logging: `tracing-journald` layer if the journal socket exists, else stderr.
- [ ] Load config. On error, show the error screen with no tiles, but still start input (Home is harmless).
- [ ] Build the `TileData` model: load icons with `slint::Image::load_from_path`, fall back to text on failure, and choose colors.
- [ ] Clock: a `slint::Timer` every 1 s; chrono `Local`; `%H:%M` or `%-I:%M %p`; date `%a %-d %b`.
- [ ] `activate(i)`: if `runner.launch()` succeeds, set `busy-text = "Opening <name>…"` and `app_active = true`.
- [ ] Runner events, posted to the UI thread:
  - clear `busy-text`; `app_active = false`;
  - set `input-locked` for 500 ms (`Timer::single_shot`);
  - toasts per the spec (5 s);
  - log the outcome.
- [ ] Input actions, posted to the UI thread:
  - `Left`/`Right` → `move-focus`; `Activate` → `activate-focused`;
  - `Home` → `runner.stop()` if active.
- [ ] Signals: SIGTERM/SIGINT through a `signal-hook`-style self-pipe, or nix's `sigwait` in a thread → `runner.stop_blocking(7s)` → `slint::quit_event_loop`.
- [ ] Headless integration with `wtype` (if cage supports virtual keyboard):
  - start with a test config of two tiles running `mpv --idle --force-window`;
  - press Right then Enter → screenshot shows mpv;
  - kill mpv → screenshot shows the launcher.

### Task 8: Deploy

**Files:** `deploy.sh`, `deploy/autologin.conf`, `deploy/bash_profile`, `README.md`.

- [ ] `deploy.sh`:
  - `set -euo pipefail`; requires root unless `DESTDIR` is set; `--restart`;
  - checks that `launcher/target/release/mushtv-launcher` exists;
  - runs `--check-config` on the repo config;
  - `install -D -m755` the binary; `install -D -m644` the assets, config (diff + `.bak` when changed), autologin override and bash_profile (`-o tv -g tv`, skipping chown under DESTDIR);
  - `systemctl daemon-reload` unless DESTDIR; restart if `--restart`.
- [ ] Dry run: `DESTDIR=$tmp ./deploy.sh` → check the tree and modes, then run again to confirm it's idempotent with no diff output.
- [ ] README: build, deploy, logs, debugging, adding a tile.

### Task 9: Verification

- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo build --release`.
- [ ] Measure RSS of the launcher under headless cage (`/proc/<pid>/status` VmRSS).
- [ ] Write the manual checklist for the real TV into README.
