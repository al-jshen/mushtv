# Steam on either GPU Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The TV's Steam tile asks "Graphics card (RTX 4060)" or "Integrated graphics card" and streams from the matching Wolf server on spc; both servers share one Steam; paused sessions end after 30 minutes.

**Architecture:** Launcher: tiles may list `[[tile.choice]]`; a picker (the Settings page component) chooses one and the runner launches its command; the last choice is remembered in prefs. spc: a systemd template runs two Wolf instances (`wolf@nvidia`, `wolf@amdgpu`) from one compose file; a Steam start-up lock keeps the shared Steam to one container; a Python service (`wolf_sessions.py`) watches both servers' event streams to end idle sessions and the other server's session on a switch.

**Tech Stack:** Rust + Slint 1.18 (launcher), bash, Docker Compose, systemd, nftables, Python 3 standard library (spc).

**Spec:** `docs/superpowers/specs/2026-10-14-steam-gpu-choice-design.md`

## Global Constraints

- Picker rows read exactly **Graphics card (RTX 4060)** and **Integrated graphics card**, no details.
- Server names: **spc** (RTX 4060, ports 47984, 47989, 47999, 48010, 48100, 48200) and **spc-igpu** (integrated graphics, each port +1000: 48984, 48989, 48999, 49010, 49100, 49200).
- Both servers use `/etc/wolf` as their state folder (shared Steam home); configs in `/etc/wolf/cfg` and `/etc/wolf/cfg-igpu`.
- Idle timeout: `WOLF_IDLE_MINUTES`, default **30**.
- Steam lock waits up to **60 s**, then the new Steam doesn't start.
- Wolf's ports answer only on `tailscale0` and `lo`.
- Work on `master`; the user commits (GPG). Commit steps below are commands to hand the user, not to run.
- Rust: `cargo fmt`, `cargo clippy --all-targets` clean; Python: standard library only.

## Review Focus

1. **Changing the mirroring mode must not forget the last GPU choice, and the reverse** (both live in one prefs file). → Task 2 test `update_keeps_other_fields`.
2. **A remembered choice that no longer exists** (renamed in `launcher.toml`) focuses the first row instead of failing. → Task 3 test `initial_focus_falls_back_to_first`.
3. **A hidden tile under the mouse pointer while the picker is open** must not launch anything. → Task 4 step guarding `on_activate`.
4. **Stopping a session that doesn't exist on the other server** (the usual case) is harmless and not logged as an action. → Task 6 `stop()` ignores non-200 replies.
5. **Wolf restarting** (event stream ends) must not leave stale idle timers or crash the service; it reconnects. → Task 6 tests `disconnect_forgets_that_servers_pauses` and the reconnect loop.

---

## File Structure

| File | Responsibility |
|---|---|
| `launcher/src/config.rs` | `Choice`; `Tile.choices`; parsing rules for `[[tile.choice]]` |
| `launcher/src/prefs.rs` | `Prefs.last_choice`; `update()` load-modify-save |
| `launcher/src/choice.rs` (new) | Pure picker logic: initial focus, moving focus |
| `launcher/src/choice_ui.rs` (new) | Picker on the UI thread: open, navigate, launch, remember |
| `launcher/src/runner.rs` | `launch_choice()` |
| `launcher/src/settings_ui.rs` | Save mirroring mode through `prefs::update` |
| `launcher/src/main.rs` | Wire picker into activation and input routing |
| `launcher/ui/main.slint` | Picker properties, keys, a second `SettingsPage` |
| `launcher/launcher.toml`, `README.md` | Steam tile with two choices; docs |
| `server/spc/compose.yaml`, `nvidia.env`, `amdgpu.env` | One compose file, per-server values |
| `server/spc/wolf@.service` (replaces `wolf.service`) | One systemd instance per GPU |
| `server/spc/wolf-prestart` | Per-instance GPU node; firewall for both; locked driver check |
| `server/spc/config.toml` | Template with `@HOSTNAME@`, `@UUID@`, Steam lock mount |
| `server/spc/steam-lock.sh` (new) | Steam start-up lock |
| `server/spc/wolf_sessions.py`, `test_wolf_sessions.py`, `wolf-sessions.service` (new) | Idle timeout and switch guard |
| `server/spc/setup.sh`, `server/spc/README.md` | Install, migration, docs |

---

### Task 1: Tile choices in the config

**Files:** Modify `launcher/src/config.rs`, `launcher/src/runner.rs` (test helper), `launcher/src/main.rs` (test helper).

**Interfaces:**
- Produces: `pub struct Choice { pub name: String, pub command: Vec<String> }`; `Tile.choices: Vec<Choice>` (empty for a plain tile; then `command` is non-empty; with choices, `command` is empty).

- [ ] **Step 1: Failing tests** in `config.rs` `mod tests`:

```rust
    const CHOICES: &str = r#"
        [[tile]]
        id = "steam"
        name = "Steam"
        [[tile.choice]]
        name = "Graphics card (RTX 4060)"
        command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc", "Steam"]
        [[tile.choice]]
        name = "Integrated graphics card"
        command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc-igpu", "Steam"]
    "#;

    #[test]
    fn tile_choices_parse_in_order() {
        let c = parse(CHOICES, &no_icons()).unwrap();
        let t = &c.tiles[0];
        assert!(t.command.is_empty());
        let names: Vec<_> = t.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Graphics card (RTX 4060)", "Integrated graphics card"]);
        assert_eq!(t.choices[1].command[4], "spc-igpu");
    }

    #[test]
    fn plain_tiles_have_no_choices() {
        assert!(parse(MINIMAL, &no_icons()).unwrap().tiles[0].choices.is_empty());
    }

    #[test]
    fn rejects_command_and_choices_together() {
        let text = CHOICES.replace("name = \"Steam\"", "name = \"Steam\"\ncommand = [\"x\"]");
        assert!(err_of(&text).contains("either command or [[tile.choice]]"));
    }

    #[test]
    fn rejects_a_tile_without_command_or_choices() {
        assert!(err_of("[[tile]]\nid = \"a\"\nname = \"A\"\n").contains("needs a command"));
    }

    #[test]
    fn rejects_a_single_choice() {
        let text = "[[tile]]\nid = \"a\"\nname = \"A\"\n[[tile.choice]]\nname = \"One\"\ncommand = [\"x\"]\n";
        assert!(err_of(text).contains("two or more"));
    }

    #[test]
    fn rejects_bad_choices() {
        let blank = CHOICES.replace("name = \"Integrated graphics card\"", "name = \"  \"");
        assert!(err_of(&blank).contains("needs a name"));
        let dup = CHOICES.replace("Integrated graphics card", "Graphics card (RTX 4060)");
        assert!(err_of(&dup).contains("duplicate choice"));
        let empty = CHOICES.replace(
            "command = [\"flatpak\", \"run\", \"com.moonlight_stream.Moonlight\", \"stream\", \"spc-igpu\", \"Steam\"]",
            "command = []",
        );
        assert!(err_of(&empty).contains("command must not be empty"));
    }
```

- [ ] **Step 2:** `cargo test --release config::` → the new tests fail to compile (`choices` unknown).

- [ ] **Step 3: Implement.** Add the type and field:

```rust
/// One way to start a tile that offers a choice (e.g. which GPU).
#[derive(Debug, Clone)]
pub struct Choice {
    pub name: String,
    pub command: Vec<String>,
}
```
In `Tile`, document `command` as "empty when the tile has choices" and add `pub choices: Vec<Choice>,`. In `RawTile`: `command: Option<Vec<String>>,` and `#[serde(default, rename = "choice")] choices: Vec<RawChoice>,`, plus:

```rust
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChoice {
    name: String,
    command: Vec<String>,
}
```
Replace the `command must not be empty` check in `parse` with:

```rust
        let (command, choices) = match (t.command, t.choices.len()) {
            (Some(_), n) if n > 0 => bail!("{what}: use either command or [[tile.choice]], not both"),
            (Some(command), _) => {
                if command.is_empty() {
                    bail!("{what}: command must not be empty");
                }
                (command, Vec::new())
            }
            (None, 0) => bail!("{what}: needs a command or two or more [[tile.choice]]"),
            (None, 1) => bail!("{what}: needs two or more [[tile.choice]] (with one, use command)"),
            (None, _) => {
                let mut choices: Vec<Choice> = Vec::new();
                for (n, c) in t.choices.into_iter().enumerate() {
                    let name = c.name.trim().to_string();
                    if name.is_empty() {
                        bail!("{what}: choice {} needs a name", n + 1);
                    }
                    if choices.iter().any(|other| other.name == name) {
                        bail!("{what}: duplicate choice {name:?}");
                    }
                    if c.command.is_empty() {
                        bail!("{what}: choice {name:?}: command must not be empty");
                    }
                    choices.push(Choice { name, command: c.command });
                }
                (Vec::new(), choices)
            }
        };
```
Icon: `resolve_icon(t.icon.as_deref(), choices.first().map_or(&command, |c| &c.command), icon_roots)`; push `Tile { …, command, choices, … }`. Add `choices: Vec::new(),` to the `Tile` literals in `runner.rs` (`fn tile`) and `main.rs` tests.

- [ ] **Step 4:** `cargo test --release` → all pass, including `repo_config_is_valid`.
- [ ] **Step 5: Commit (user):** `git add launcher/src && git commit -m "Launcher: tiles can offer a choice of commands"`

---

### Task 2: Remembered choices in prefs, without clobbering

**Files:** Modify `launcher/src/prefs.rs`, `launcher/src/settings_ui.rs`.

**Interfaces:**
- Produces: `Prefs.last_choice: BTreeMap<String, String>` (tile id → choice name); `pub fn update(path: &Path, change: impl FnOnce(&mut Prefs)) -> Result<()>`.

- [ ] **Step 1: Failing tests:**

```rust
    #[test]
    fn last_choice_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        let mut prefs = Prefs::default();
        prefs.last_choice.insert("steam".into(), "Integrated graphics card".into());
        save(&p, &prefs).unwrap();
        assert_eq!(load(&p), prefs);
    }

    #[test]
    fn update_keeps_other_fields() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        update(&p, |p| { p.last_choice.insert("steam".into(), "Integrated graphics card".into()); }).unwrap();
        update(&p, |p| p.mirroring_mode = MirrorMode::Smooth).unwrap();
        let prefs = load(&p);
        assert_eq!(prefs.mirroring_mode, MirrorMode::Smooth);
        assert_eq!(prefs.last_choice["steam"], "Integrated graphics card");
    }
```
Change `fn smooth()` to `Prefs { mirroring_mode: MirrorMode::Smooth, ..Prefs::default() }`.

- [ ] **Step 2:** `cargo test --release prefs::` → fails to compile.
- [ ] **Step 3: Implement:** `use std::collections::BTreeMap;`, field `pub last_choice: BTreeMap<String, String>,` (struct already has `#[serde(default)]`), and

```rust
/// Changes some prefs and saves them, keeping the rest of the file.
pub fn update(path: &Path, change: impl FnOnce(&mut Prefs)) -> Result<()> {
    let mut prefs = load(path);
    change(&mut prefs);
    save(path, &prefs)
}
```
In `settings_ui.rs` `Effect::SetMirroringMode`, replace `prefs::save(path, &Prefs { mirroring_mode: mode })` with `prefs::update(path, |p| p.mirroring_mode = mode)` (drop the now-unused `Prefs` import if clippy says so).
- [ ] **Step 4:** `cargo test --release && cargo clippy --release --all-targets` → clean.
- [ ] **Step 5: Commit (user):** `git add launcher/src && git commit -m "Launcher: remember choices; prefs updates keep other settings"`

---

### Task 3: Picker logic and launching a choice

**Files:** Create `launcher/src/choice.rs`; modify `launcher/src/runner.rs`, `launcher/src/main.rs` (`mod choice;`).

**Interfaces:**
- Produces: `choice::initial_focus(names: &[&str], remembered: Option<&str>) -> usize`; `choice::step(focused: usize, delta: i32, len: usize) -> usize`; `Runner::launch_choice(&self, tile_index: usize, tile: &Tile, choice: usize) -> bool`.

- [ ] **Step 1: Failing tests** (`choice.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 2] = ["Graphics card (RTX 4060)", "Integrated graphics card"];

    #[test]
    fn initial_focus_uses_the_remembered_choice() {
        assert_eq!(initial_focus(&NAMES, Some("Integrated graphics card")), 1);
    }

    #[test]
    fn initial_focus_falls_back_to_first() {
        assert_eq!(initial_focus(&NAMES, None), 0);
        assert_eq!(initial_focus(&NAMES, Some("Renamed since")), 0);
    }

    #[test]
    fn step_stays_in_range() {
        assert_eq!(step(0, -1, 2), 0);
        assert_eq!(step(0, 1, 2), 1);
        assert_eq!(step(1, 1, 2), 1);
        assert_eq!(step(0, 1, 0), 0);
    }
}
```
Runner test:

```rust
    #[test]
    fn launch_choice_runs_the_chosen_command() {
        let (r, rx) = runner();
        let mut t = tile(&["sh", "-c", "exit 9"], None);
        t.command.clear();
        t.choices = vec![
            Choice { name: "a".into(), command: vec!["sh".into(), "-c".into(), "exit 0".into()] },
            Choice { name: "b".into(), command: vec!["sh".into(), "-c".into(), "exit 3".into()] },
        ];
        assert!(r.launch_choice(4, &t, 1));
        // Same event shape as `launch`: App::Tile(4), then an exit with code 3.
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ev.app, App::Tile(4));
        assert!(!r.launch_choice(4, &t, 7)); // no such choice
    }
```
(Match the event assertions to the existing `launch` exit-code test in `runner.rs`, e.g. the `exit 3` case at the `r.launch(2, …)` test; use the same fields it checks.)

- [ ] **Step 2:** `cargo test --release choice:: runner::` → fails to compile.
- [ ] **Step 3: Implement** `choice.rs`:

```rust
//! Choosing how to start a tile that offers choices (e.g. which GPU):
//! where focus starts and how it moves. Pure; the UI glue is `choice_ui`.

/// The remembered choice if it still exists, else the first.
pub fn initial_focus(names: &[&str], remembered: Option<&str>) -> usize {
    remembered
        .and_then(|r| names.iter().position(|n| *n == r))
        .unwrap_or(0)
}

/// Moves focus by `delta`, staying on the list.
pub fn step(focused: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focused as i64 + i64::from(delta)).clamp(0, len as i64 - 1) as usize
}
```
In `runner.rs` (`use crate::config::Choice` only in tests if needed):

```rust
    /// Starts one of a tile's choices; the tile's id, stop command and
    /// events are as for [`Runner::launch`]. False if busy or no such choice.
    pub fn launch_choice(&self, tile_index: usize, tile: &Tile, choice: usize) -> bool {
        let Some(choice) = tile.choices.get(choice) else {
            return false;
        };
        self.start(
            App::Tile(tile_index),
            &tile.id,
            &with_die_with_parent(&choice.command),
            tile.stop_command.clone(),
            None,
        )
    }
```
- [ ] **Step 4:** `cargo test --release` → pass.
- [ ] **Step 5: Commit (user):** `git add launcher/src && git commit -m "Launcher: picker logic and launching a choice"`

---

### Task 4: The picker on screen

**Files:** Create `launcher/src/choice_ui.rs`; modify `launcher/src/main.rs`, `launcher/ui/main.slint`.

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: `choice_ui::init(ui, runner: Arc<Runner>, input: Arc<OnceLock<InputHandle>>, tiles: Arc<Vec<Tile>>, prefs_path: Option<PathBuf>)`, `choice_ui::open(tile: usize)`, `choice_ui::is_open() -> bool`, `choice_ui::nav(ChoiceNav)` with `pub enum ChoiceNav { Up, Down, Select, Back }`; Slint: `choice-open`, `choice-title`, `choice-rows`, `choice-focused`, callbacks `choice-nav(int)` (0 up, 1 down, 2 select, 3 back), `choice-hover(int)`, `choice-click(int)`.

- [ ] **Step 1: Slint.** In `main.slint` add, next to the Settings properties:

```slint
    // The picker for a tile with choices (drawn with the Settings page).
    in property <bool> choice-open;
    in property <string> choice-title;
    in property <[SettingsRow]> choice-rows;
    in property <int> choice-focused;
    // 0 up, 1 down, 2 select, 3 back.
    callback choice-nav(int);
    callback choice-hover(int);
    callback choice-click(int);
```
Add `|| root.choice-open` to `home-opacity`. In the `FocusScope`, before the `settings-open` branch:

```slint
            if root.choice-open {
                if root.input-locked {
                    return accept;
                }
                if event.text == Key.UpArrow {
                    root.choice-nav(0);
                } else if event.text == Key.DownArrow {
                    root.choice-nav(1);
                } else if event.text == Key.Return || event.text == "\n" || event.text == "\u{d}" || event.text == Key.Space {
                    root.choice-nav(2);
                } else if event.text == Key.Escape || event.text == Key.Backspace {
                    root.choice-nav(3);
                } else {
                    return reject;
                }
                return accept;
            }
```
Add `&& !root.choice-open` to both tile `hovered`/`clicked` guards. After the Settings page:

```slint
    if root.choice-open: SettingsPage {
        width: parent.width;
        height: parent.height;
        u: root.u;
        title: root.choice-title;
        status: "";
        rows: root.choice-rows;
        focused: root.choice-focused;
        row-hovered(i) => {
            if !root.input-locked {
                root.choice-hover(i);
            }
        }
        row-clicked(i) => {
            if !root.input-locked {
                root.choice-click(i);
            }
        }
    }
```

- [ ] **Step 2: `choice_ui.rs`** (same thread-local pattern as `mirroring_ui`):

```rust
//! The picker for a tile with choices (e.g. which GPU to stream Steam from):
//! shown with the Settings page, it starts the chosen command and remembers
//! the choice. Runs on the UI thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::config::Tile;
use crate::input::InputHandle;
use crate::runner::Runner;
use crate::{MainWindow, SettingsRow, choice, prefs};

/// Material Icons "play_arrow", on every row.
const ROW_GLYPH: &str = "\u{e037}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceNav {
    Up,
    Down,
    Select,
    Back,
}

struct Ctl {
    weak: Weak<MainWindow>,
    runner: Arc<Runner>,
    input: Arc<OnceLock<InputHandle>>,
    tiles: Arc<Vec<Tile>>,
    prefs_path: Option<PathBuf>,
    /// (tile index, focused row) while open.
    open: Option<(usize, usize)>,
}

thread_local! {
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ctl) -> R) -> Option<R> {
    CTL.with(|ctl| ctl.borrow_mut().as_mut().map(f))
}

pub fn init(
    ui: &MainWindow,
    runner: Arc<Runner>,
    input: Arc<OnceLock<InputHandle>>,
    tiles: Arc<Vec<Tile>>,
    prefs_path: Option<PathBuf>,
) {
    ui.on_choice_nav(|code| {
        nav(match code {
            0 => ChoiceNav::Up,
            1 => ChoiceNav::Down,
            2 => ChoiceNav::Select,
            _ => ChoiceNav::Back,
        })
    });
    ui.on_choice_hover(|row| with(|ctl| ctl.focus(usize::try_from(row).unwrap_or(0))).unwrap_or(()));
    ui.on_choice_click(|row| {
        with(|ctl| ctl.focus(usize::try_from(row).unwrap_or(0)));
        nav(ChoiceNav::Select);
    });
    CTL.with(|ctl| {
        *ctl.borrow_mut() = Some(Ctl { weak: ui.as_weak(), runner, input, tiles, prefs_path, open: None })
    });
}

pub fn is_open() -> bool {
    with(|ctl| ctl.open.is_some()).unwrap_or(false)
}

/// Opens the picker for `tile`, focused on the choice used last.
pub fn open(tile: usize) {
    with(|ctl| {
        let Some(t) = ctl.tiles.get(tile) else { return };
        let names: Vec<&str> = t.choices.iter().map(|c| c.name.as_str()).collect();
        let remembered = ctl.prefs_path.as_deref().map(prefs::load).and_then(|p| p.last_choice.get(&t.id).cloned());
        ctl.open = Some((tile, choice::initial_focus(&names, remembered.as_deref())));
        ctl.render();
    });
}

pub fn nav(nav: ChoiceNav) {
    with(|ctl| ctl.nav(nav));
}

impl Ctl {
    fn focus(&mut self, row: usize) {
        if let Some((tile, _)) = self.open {
            let len = self.tiles[tile].choices.len();
            self.open = Some((tile, row.min(len.saturating_sub(1))));
            self.render();
        }
    }

    fn nav(&mut self, nav: ChoiceNav) {
        let Some((tile, focused)) = self.open else { return };
        let len = self.tiles[tile].choices.len();
        match nav {
            ChoiceNav::Up => self.open = Some((tile, choice::step(focused, -1, len))),
            ChoiceNav::Down => self.open = Some((tile, choice::step(focused, 1, len))),
            ChoiceNav::Back => self.open = None,
            ChoiceNav::Select => {
                self.open = None;
                self.start(tile, focused);
            }
        }
        self.render();
    }

    fn start(&self, tile: usize, choice: usize) {
        let t = &self.tiles[tile];
        let Some(ui) = self.weak.upgrade() else { return };
        if !self.runner.launch_choice(tile, t, choice) {
            return;
        }
        let name = &t.choices[choice].name;
        tracing::info!("opening {} on {name} ({})", t.name, t.choices[choice].command.join(" "));
        crate::opening(&ui, self.input.get(), &t.name);
        if let Some(path) = &self.prefs_path {
            let (id, name) = (t.id.clone(), name.clone());
            if let Err(e) = prefs::update(path, |p| {
                p.last_choice.insert(id, name);
            }) {
                tracing::warn!("prefs: {e:#}");
            }
        }
    }

    fn render(&self) {
        let Some(ui) = self.weak.upgrade() else { return };
        ui.set_choice_open(self.open.is_some());
        let Some((tile, focused)) = self.open else { return };
        let t = &self.tiles[tile];
        let rows: Vec<SettingsRow> = t
            .choices
            .iter()
            .map(|c| SettingsRow { icon: ROW_GLYPH.into(), label: c.name.as_str().into(), detail: "".into(), checked: false })
            .collect();
        ui.set_choice_title(t.name.as_str().into());
        ui.set_choice_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        ui.set_choice_focused(i32::try_from(focused).unwrap_or(0));
    }
}
```

- [ ] **Step 3: `main.rs`.** `mod choice; mod choice_ui;`. Extract the "opening" lines into:

```rust
/// After an app was started: "Opening …" and only Home gets through.
fn opening(ui: &MainWindow, input: Option<&InputHandle>, name: &str) {
    ui.set_busy_text(format!("Opening {name}…").into());
    if let Some(input) = input {
        input.set_app_active(true);
    }
}
```
and use it in `on_activate` (`opening(&ui, Some(&input), &tile.name)`). In `on_activate`: extend the hidden-tile guard to `settings_ui::is_open() || mirroring_ui::is_open() || choice_ui::is_open()`; before `runner.launch`, `if !tile.choices.is_empty() { choice_ui::open(index); return; }`. After `mirroring_ui::init(…)`, call `choice_ui::init(&ui, Arc::clone(&runner), Arc::clone(&input_slot), Arc::clone(&tiles), prefs_path.clone())` (keep `prefs_path` available: pass a clone to `settings_ui::init`). In `on_action`, before the Settings branch:

```rust
    if choice_ui::is_open() {
        match action {
            Action::Up => choice_ui::nav(choice_ui::ChoiceNav::Up),
            Action::Down => choice_ui::nav(choice_ui::ChoiceNav::Down),
            Action::Activate => choice_ui::nav(choice_ui::ChoiceNav::Select),
            Action::Back | Action::Home => choice_ui::nav(choice_ui::ChoiceNav::Back),
            Action::Left | Action::Right => {}
        }
        return;
    }
```
- [ ] **Step 4:** `cargo build --release && cargo test --release && cargo clippy --release --all-targets && cargo fmt --check` → clean.
- [ ] **Step 5: Commit (user):** `git add launcher && git commit -m "Launcher: picker for tiles with choices"`

---

### Task 5: Steam tile with two choices; screenshots; docs

**Files:** Modify `launcher/launcher.toml`, `README.md`.

- [ ] **Step 1:** Replace the Steam tile's `command` with the two `[[tile.choice]]` entries from the spec (comment above the tile explains both servers). Update the config header comment's field list with `choice`.
- [ ] **Step 2:** `./target/release/mushtv-launcher --check-config launcher.toml` → `OK, 4 tiles`.
- [ ] **Step 3: Headless.** Scenario (focus Steam with `Right Right`, `Return`, `shot picker`, `Down`, `shot picker-down`, `Escape`, `shot back-home`) with a test copy of the config whose Steam choices run `["sleep", "30"]` and `["sleep", "31"]`, and `XDG_STATE_HOME` set to a temp dir; a second run after `Return` on row 2 then `Escape`-less exit checks the picker reopens focused on row 2 (`shot remembered`). Inspect the screenshots.
- [ ] **Step 4: README:** Games section: the picker and both servers; checklist lines for the picker and switching.
- [ ] **Step 5: Commit (user):** `git add README.md launcher && git commit -m "Steam tile: choose the GPU"`

---

### Task 6: Session guard (`wolf_sessions.py`)

**Files:** Create `server/spc/wolf_sessions.py`, `server/spc/test_wolf_sessions.py`, `server/spc/wolf-sessions.service`.

**Interfaces:**
- Produces: `Tracker(servers, idle_seconds)` with `on_event(server, kind, session_id, now) -> list[(server, session_id)]`, `on_disconnect(server)`, `due(now) -> list[(server, session_id)]`; `event_kind(type) -> "start"|"pause"|"resume"|"stop"|None`; `sse_events(lines)`; `session_id_of(data) -> str|None`.

- [ ] **Step 1: Failing tests** (`test_wolf_sessions.py`, run with `python3 -m unittest -v test_wolf_sessions` from `server/spc`):

```python
import io
import unittest

from wolf_sessions import Tracker, event_kind, session_id_of, sse_events

SERVERS = ["nvidia", "amdgpu"]
TV = "5334278758390175681"


class TrackerTest(unittest.TestCase):
    def setUp(self):
        self.t = Tracker(SERVERS, idle_seconds=1800)

    def test_pause_ends_after_the_idle_time_once(self):
        self.t.on_event("amdgpu", "pause", TV, now=0)
        self.assertEqual(self.t.due(now=1799), [])
        self.assertEqual(self.t.due(now=1800), [("amdgpu", TV)])
        self.assertEqual(self.t.due(now=5000), [])

    def test_resume_cancels(self):
        self.t.on_event("amdgpu", "pause", TV, now=0)
        self.t.on_event("amdgpu", "resume", TV, now=10)
        self.assertEqual(self.t.due(now=9999), [])

    def test_stop_cancels(self):
        self.t.on_event("nvidia", "pause", TV, now=0)
        self.t.on_event("nvidia", "stop", TV, now=5)
        self.assertEqual(self.t.due(now=9999), [])

    def test_start_or_resume_ends_the_other_servers_session(self):
        self.assertEqual(self.t.on_event("amdgpu", "start", TV, now=0), [("nvidia", TV)])
        self.assertEqual(self.t.on_event("nvidia", "resume", TV, now=1), [("amdgpu", TV)])
        self.assertEqual(self.t.on_event("nvidia", "pause", TV, now=2), [])

    def test_disconnect_forgets_that_servers_pauses(self):
        self.t.on_event("nvidia", "pause", TV, now=0)
        self.t.on_event("amdgpu", "pause", "42", now=0)
        self.t.on_disconnect("nvidia")
        self.assertEqual(self.t.due(now=9999), [("amdgpu", "42")])


class ParsingTest(unittest.TestCase):
    def test_event_kind(self):
        self.assertEqual(event_kind("wolf::core::events::StreamSession"), "start")
        self.assertEqual(event_kind("wolf::core::events::PauseStreamEvent"), "pause")
        self.assertEqual(event_kind("wolf::core::events::ResumeStreamEvent"), "resume")
        self.assertEqual(event_kind("wolf::core::events::StopStreamEvent"), "stop")
        self.assertIsNone(event_kind("wolf::core::events::VideoSession"))
        self.assertIsNone(event_kind("PairSignal"))

    def test_session_id_of(self):
        self.assertEqual(session_id_of('{"session_id": 5334278758390175681}'), TV)
        self.assertEqual(session_id_of('{"session_id": "42"}'), "42")
        self.assertIsNone(session_id_of('{"other": 1}'))
        self.assertIsNone(session_id_of("not json"))
        self.assertIsNone(session_id_of("[1, 2]"))

    def test_sse_events(self):
        stream = io.BytesIO(
            b"event: A\ndata: {\"x\": 1}\n\n"
            b": comment\n"
            b"event: B\r\ndata: 2\r\n\r\n"
            b"event: C\ndata: unterminated"
        )
        self.assertEqual(list(sse_events(stream)), [("A", '{"x": 1}'), ("B", "2")])


if __name__ == "__main__":
    unittest.main()
```
- [ ] **Step 2:** `cd server/spc && python3 -m unittest -v test_wolf_sessions` → `ModuleNotFoundError`.
- [ ] **Step 3: Implement `wolf_sessions.py`:**

```python
#!/usr/bin/env python3
"""Ends idle Wolf sessions and keeps the shared Steam to one server at a time.

Both Wolf servers on spc (wolf@nvidia: "spc", wolf@amdgpu: "spc-igpu") share
each device's Steam home. This service watches their event streams:
- a session paused for WOLF_IDLE_MINUTES (default 30) is ended;
- a session starting or resuming on one server ends the same session (the same
  device) on the other.
Run by wolf-sessions.service; logs go to the journal.
"""

import json
import os
import queue
import socket
import threading
import time

SERVERS = {
    "nvidia": "/run/wolf/nvidia/sockets/wolf.sock",
    "amdgpu": "/run/wolf/amdgpu/sockets/wolf.sock",
}

KINDS = {
    "StreamSession": "start",
    "PauseStreamEvent": "pause",
    "ResumeStreamEvent": "resume",
    "StopStreamEvent": "stop",
}


def log(msg):
    print(msg, flush=True)


def event_kind(event_type):
    """'wolf::core::events::PauseStreamEvent' -> 'pause'; None for others."""
    return KINDS.get(event_type.rsplit("::", 1)[-1].strip())


def session_id_of(data):
    """The session_id in an event's JSON, as a string (they exceed 2^53)."""
    try:
        value = json.loads(data)
    except ValueError:
        return None
    if not isinstance(value, dict) or value.get("session_id") is None:
        return None
    return str(value["session_id"])


def sse_events(lines):
    """(event, data) pairs from a Server-Sent Events byte stream."""
    event, data = None, []
    for raw in lines:
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if not line:
            if event is not None:
                yield event, "\n".join(data)
            event, data = None, []
        elif line.startswith("event:"):
            event = line[len("event:"):].strip()
        elif line.startswith("data:"):
            data.append(line[len("data:"):].strip())


class Tracker:
    """Decides which sessions to end. No I/O; time is passed in (seconds)."""

    def __init__(self, servers, idle_seconds):
        self.servers = list(servers)
        self.idle = idle_seconds
        self.paused = {}  # (server, session_id) -> when it paused

    def on_event(self, server, kind, session_id, now):
        """Returns (server, session_id) pairs to end now."""
        key = (server, session_id)
        if kind == "pause":
            self.paused[key] = now
            return []
        self.paused.pop(key, None)
        if kind in ("start", "resume"):
            return [(other, session_id) for other in self.servers if other != server]
        return []

    def on_disconnect(self, server):
        """The server's event stream broke (e.g. Wolf restarted): its sessions are gone."""
        for key in [k for k in self.paused if k[0] == server]:
            del self.paused[key]

    def due(self, now):
        """Sessions paused for the idle time; each is returned once."""
        expired = [k for k, t in self.paused.items() if now - t >= self.idle]
        for key in expired:
            del self.paused[key]
        return expired


def request(sock_path, method, path, body=b""):
    """One HTTP request over Wolf's Unix socket; returns (status, body)."""
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(15)
        s.connect(sock_path)
        s.sendall(
            f"{method} {path} HTTP/1.0\r\nHost: localhost\r\n"
            f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n".encode() + body
        )
        f = s.makefile("rb")
        status = int(f.readline().split()[1])
        length = None
        while (line := f.readline()) not in (b"\r\n", b"\n", b""):
            name, _, value = line.decode("latin-1").partition(":")
            if name.strip().lower() == "content-length":
                length = int(value)
        return status, (f.read(length) if length is not None else f.read()).decode("utf-8", "replace")


def stop(server, session_id, why):
    try:
        status, body = request(
            SERVERS[server], "POST", "/api/v1/sessions/stop",
            json.dumps({"session_id": session_id}).encode(),
        )
    except OSError as e:
        log(f"{server}: couldn't end session {session_id}: {e}")
        return
    if status == 200 and '"success":true' in body.replace(" ", ""):
        log(f"{server}: ended session {session_id} ({why})")
    # Otherwise it wasn't running there, the usual case after a switch.


def watch(server, sock_path, events):
    """Feeds (server, kind, session_id) into `events`; reconnects forever."""
    delay, connected = 1, None
    while True:
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.connect(sock_path)
                s.sendall(b"GET /api/v1/events HTTP/1.0\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n")
                f = s.makefile("rb")
                while f.readline() not in (b"\r\n", b"\n", b""):
                    pass
                if connected is not True:
                    log(f"{server}: watching events")
                connected, delay = True, 1
                for event_type, data in sse_events(f):
                    kind = event_kind(event_type)
                    session_id = session_id_of(data) if kind else None
                    if session_id:
                        events.put((server, kind, session_id))
        except OSError as e:
            if connected is not False:
                log(f"{server}: no event stream ({e}); retrying")
        connected = False
        events.put((server, "disconnect", None))
        time.sleep(delay)
        delay = min(delay * 2, 30)


def main():
    idle = float(os.environ.get("WOLF_IDLE_MINUTES", "30")) * 60
    tracker = Tracker(SERVERS, idle)
    events = queue.Queue()
    for server, path in SERVERS.items():
        threading.Thread(target=watch, args=(server, path, events), daemon=True).start()
    while True:
        try:
            server, kind, session_id = events.get(timeout=30)
        except queue.Empty:
            pass
        else:
            if kind == "disconnect":
                tracker.on_disconnect(server)
            else:
                for other, sid in tracker.on_event(server, kind, session_id, time.monotonic()):
                    stop(other, sid, f"started on {server}")
        for server, sid in tracker.due(time.monotonic()):
            stop(server, sid, f"paused for {idle / 60:g} min")


if __name__ == "__main__":
    main()
```
- [ ] **Step 4:** `python3 -m unittest -v test_wolf_sessions` → all pass. `wolf-sessions.service`:

```ini
[Unit]
Description=Ends idle Wolf sessions; one shared Steam at a time
After=wolf@nvidia.service wolf@amdgpu.service

[Service]
ExecStart=/usr/bin/python3 /opt/wolf/wolf_sessions.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```
`systemd-analyze verify` it (ignore the missing `/opt/wolf` path).
- [ ] **Step 5: Commit (user):** `git add server/spc && git commit -m "spc: session guard for idle and switched sessions"`

---

### Task 7: Two Wolf servers, Steam lock, migration

**Files:** Modify `server/spc/compose.yaml`, `wolf-prestart`, `config.toml`, `setup.sh`, `README.md`; create `nvidia.env`, `amdgpu.env`, `wolf@.service`, `steam-lock.sh`; delete `wolf.service`.

- [ ] **Step 1: Per-server values.** `nvidia.env`:

```sh
# Wolf on the RTX 4060: Moonlight sees it as "spc". Read by compose (see wolf@.service).
WOLF_INSTANCE=nvidia
WOLF_CONTAINER=wolf
WOLF_CFG_DIR=/etc/wolf/cfg
WOLF_HTTPS_PORT=47984
WOLF_HTTP_PORT=47989
WOLF_CONTROL_PORT=47999
WOLF_RTSP_SETUP_PORT=48010
WOLF_VIDEO_PING_PORT=48100
WOLF_AUDIO_PING_PORT=48200
```
`amdgpu.env`: same keys with `amdgpu`, `wolf-igpu`, `/etc/wolf/cfg-igpu`, and 48984, 48989, 48999, 49010, 49100, 49200; comment "Moonlight sees it as spc-igpu (pair as spc:48989)".
- [ ] **Step 2: `compose.yaml`:** `container_name: ${WOLF_CONTAINER}`; `env_file: [/run/wolf/${WOLF_INSTANCE}/render.env]`; environment adds `WOLF_CFG_FILE: ${WOLF_CFG_DIR}/config.toml`, `WOLF_PRIVATE_KEY_FILE: ${WOLF_CFG_DIR}/key.pem`, `WOLF_PRIVATE_CERT_FILE: ${WOLF_CFG_DIR}/cert.pem` and the six `WOLF_*_PORT: ${WOLF_*_PORT}`; the runtime mount becomes `/run/wolf/${WOLF_INSTANCE}/sockets:/run/user/wolf:rw`. Drop the top-level `name:` (the unit passes `-p`).
- [ ] **Step 3: `wolf@.service`:**

```ini
[Unit]
Description=Wolf game streaming server on the %i GPU
Requires=docker.service
After=docker.service network-online.target tailscaled.service
Wants=network-online.target

[Service]
WorkingDirectory=/opt/wolf
ExecStartPre=/opt/wolf/wolf-prestart %i
ExecStart=/usr/bin/docker compose -p wolf-%i --env-file /opt/wolf/%i.env up
ExecStop=/usr/bin/docker compose -p wolf-%i --env-file /opt/wolf/%i.env down
Restart=on-failure
RestartSec=10
# Rebuilding the driver volume after a driver update downloads and unpacks
# NVIDIA's installer.
TimeoutStartSec=30min

[Install]
WantedBy=multi-user.target
```
- [ ] **Step 4: `wolf-prestart <instance>`:** `gpu=${1:?usage: wolf-prestart nvidia|amdgpu}` replaces `WOLF_GPU`; the firewall table drops `tcp dport { 47984, 47989, 48010, 48984, 48989, 49010 }` and `udp dport { 47999, 48100, 48200, 48999, 49100, 49200 }`; `install -d -m 755 /run/wolf/$gpu /run/wolf/$gpu/sockets` and `echo "WOLF_RENDER_NODE=$node" >/run/wolf/$gpu/render.env`; wrap the driver-volume step in `exec 8>/run/lock/wolf-driver.lock; flock 8` (both instances start together at boot).
- [ ] **Step 5: `steam-lock.sh`:**

```bash
# mushtv: sourced as root by the Steam container's entrypoint before Steam
# starts (mounted as /etc/cont-init.d/05-mushtv-steam-lock.sh). Both Wolf
# servers share this Steam home, so wait for the other server's Steam to
# exit. The lock stays held on descriptor 9, which Steam inherits, until this
# container exits.
exec 9>>/home/retro/.mushtv-steam.lock
if ! flock -n 9; then
	gow_log "[mushtv] waiting up to 60 s for the other server's Steam to close"
	if ! flock -w 60 9; then
		gow_log "[mushtv] the other server's Steam is still running; not starting"
		exit 1
	fi
fi
```
- [ ] **Step 6: `config.toml` template:** header explains placeholders; `hostname = "@HOSTNAME@"`, add `uuid = "@UUID@"` after `config_version`; the Steam app's `mounts = ["/opt/wolf/steam-lock.sh:/etc/cont-init.d/05-mushtv-steam-lock.sh:ro"]`.
- [ ] **Step 7: `setup.sh`:** remove `--gpu` (usage: no arguments). Install `compose.yaml nvidia-driver.Dockerfile nvidia.env amdgpu.env steam-lock.sh wolf_sessions.py` (644) and `wolf-prestart` (755) to `/opt/wolf`, `wolf@.service wolf-sessions.service` to `/etc/systemd/system`. Migration: if `/etc/systemd/system/wolf.service` exists, `systemctl disable --now wolf.service`, remove it and `/etc/systemd/system/wolf.service.d`. Configs:

```bash
# Each server's config: from the template, with its name and a server ID.
# A config without the Steam lock predates two servers: keep it as .bak and
# replace it (devices then pair again; Steam's folder is untouched).
config_for() { # dir hostname
	local cfg="$1/config.toml"
	install -d -m 700 "$1"
	if [ -e "$cfg" ] && grep -q steam-lock "$cfg"; then
		return
	fi
	[ -e "$cfg" ] && cp -p "$cfg" "$cfg.bak"
	sed -e "s/@HOSTNAME@/$2/" -e "s/@UUID@/$(cat /proc/sys/kernel/random/uuid)/" config.toml >"$cfg"
	chmod 600 "$cfg"
}
config_for /etc/wolf/cfg spc
config_for /etc/wolf/cfg-igpu spc-igpu
```
Then `systemctl daemon-reload`, `systemctl enable wolf@nvidia wolf@amdgpu wolf-sessions`, `systemctl restart wolf@nvidia wolf@amdgpu wolf-sessions`, status of all three.
- [ ] **Step 8: Checks (local):** `bash -n`; `docker compose config` with each env file (render.env pointed at a temp file); render the template with `sed` and parse it with `python3 -c 'import tomllib,sys; tomllib.load(open(sys.argv[1],"rb"))'`; the nft block loads twice in an `alpine` container; `systemd-analyze verify` both units; `bash -n steam-lock.sh` (with a stub `gow_log`), and a local flock test: hold the lock in one shell, source the script in another with `flock -w 2` substituted, expect exit 1.
- [ ] **Step 9: README (`server/spc`):** two servers, ports, the shared Steam and lock, the session guard, migration and re-pairing; drop `--gpu`.
- [ ] **Step 10: Commit (user):** `git add -A server/spc && git commit -m "spc: two Wolf servers sharing Steam"`

---

### Task 8: Roll out on spc and pair

- [ ] **Step 1:** `rsync -a --delete server/spc/ jshen@spc:wolf-setup/`; user runs `ssh -t jshen@spc sudo bash wolf-setup/setup.sh`.
- [ ] **Step 2: Verify:** `systemctl is-active wolf@nvidia wolf@amdgpu wolf-sessions`; `docker logs` of each shows its encoder (`nvcodec`, `va`) and HTTP port; `curl http://spc:47989/serverinfo` and `:48989` answer with hostnames `spc`/`spc-igpu` and non-empty `uniqueid`; the LAN address times out on both ports; `journalctl` isn't readable as jshen, so check the guard with `docker`-free means: `systemctl status wolf-sessions` ("watching events" twice).
- [ ] **Step 3: Pair** the test client, then the TV (user, as `tv`), for `spc` and `spc:48989`, approving PINs with the watcher script (two runs, one per server's log: `docker logs wolf` / `docker logs wolf-igpu`).
- [ ] **Step 4: Check `StreamSession`'s JSON** by subscribing to events (curl container on the socket) during a test stream; confirm it has `session_id`. If not, switch the "start" trigger to `DockerContainerCreated` (it has `session_id` as a string) in `KINDS` and its test.

### Task 9: End-to-end tests on spc, then cleanup

- [ ] **Step 1: Switch:** test client streams Steam from `spc-igpu`, disconnects (pause), then streams from `spc`: the old `WolfSteam_*` container is gone (Wolf force-recreates by name; the guard ends the `spc-igpu` session), Steam starts on `spc`, and each server's `<state>/<hash>/Steam/udev` belongs to its own server (a controller works after the switch, checked on the TV). (Updated after the final review: the lock is a backstop, not the switch mechanism.)
- [ ] **Step 2: Idle:** run a second guard in a container with `WOLF_IDLE_MINUTES=1` (`docker run --rm --user 0 -v /run/wolf:/run/wolf -v /opt/wolf:/opt/wolf:ro -e WOLF_IDLE_MINUTES=1 python:3.13-alpine python /opt/wolf/wolf_sessions.py`); pause a test session; it ends within ~1.5 min.
- [ ] **Step 3: Cleanup:** quit and unpair the test client (its pairing is gone with the replaced configs; delete `/etc/wolf/5711466711918555371` with sudo, user); `docker builder prune -af`; remove `ubuntu:22.04`, `curlimages/curl:8.11.1`, `python:3.13-alpine` and `gow/nvidia-driver:*` images; remove `~/.config/sway/custom-cfg` from the TV's Steam home (sudo, user).
- [ ] **Step 4:** Deploy the launcher (`sudo ./deploy.sh --restart`, user) and hand the TV checklist to the user.
