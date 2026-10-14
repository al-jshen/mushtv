# Mirroring: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A built-in Mirroring tile and screen that runs UxPlay on demand, shows the pairing code, and returns to the tiles when mirroring ends. Plus a Settings option for low lag versus smooth playback, saved in a new prefs file.

**Architecture:** Pure `mirroring` (command and output parser) and `prefs` modules carry the logic. The runner gains `launch_capturing` (piped output, same lifecycle). A UI-thread `mirroring_ui` module glues the parsed events to the screen and to input gating.

**Tech stack:** existing crate; no new dependencies (`serde`/`toml` for prefs).

**Spec:** `docs/superpowers/specs/2026-10-14-mirroring-design.md`

**Execution:** Native (the user asked to build straight away).

## Global constraints

- UxPlay runs only while the Mirroring screen is open, and always through the runner (process group, Home, pdeathsig).
- UxPlay always runs with `-pin`. Low lag (`-vsync no`) is the default mode.
- UxPlay runs under `stdbuf -oL -eL`, so output arrives line by line.
- The prefs file lives in tv's home and is written atomically. A failure never crashes the launcher.

## Review focus

1. **The local probe from Home Assistant or Music Assistant arrives while waiting.** It must not close the screen or show a code. Test: the fixture's probe lines produce no events.
2. **The Mac cancels at the code prompt.** The code stays shown until Back or Home, and nothing crashes. Test: CodeRequested then nothing; `back()` stops it.
3. **UxPlay exits right away** (not installed, ports busy). The toast appears, the screen closes, and the input gate reopens. Test: the runner `quick` path for `App::Mirroring` (`sh -c 'exit 1'`).
4. **A corrupt or unwritable prefs file.** Defaults are used, and a toast appears on save failure. Tests in `prefs`.
5. **Back pressed on the gamepad while mirroring.** It's ignored (only Home). Test: logic gating with app active.

---

### Task 1: `mirroring` module (command + parser)
Tests first, using `tests/fixtures/uxplay-session.log`:
- [ ] `command("mushtv", LowLag)` exactly matches the spec, with `-vsync no`.
- [ ] `command(.., Smooth)` has no `-vsync no`.
- [ ] Parsing the whole fixture yields exactly `[CodeRequested("7932"), Connected("Living Room Mac"), Started, Stopped]`.
- [ ] Junk lines, the probe lines and "Connection closed" give `None`.
- [ ] A code line with other digits and spacing variants is handled.
- [ ] `MirrorMode` serde: "low-lag" / "smooth".

### Task 2: `prefs` module
- [ ] Missing file → default. Corrupt file → default. Unknown keys are ignored (forward compatible).
- [ ] Save then load round-trips. Save creates parent directories. Save is atomic (temporary file, then rename).
- [ ] Save into an unwritable directory → Err.
- [ ] `default_path()` honours `XDG_STATE_HOME`, else `$HOME/.local/state/mushtv/settings.toml`.

### Task 3: runner `launch_capturing` + `App`
- [ ] `RunEvent` carries `app: App`. Existing tests are updated.
- [ ] Test: `sh -c 'echo one; echo two >&2; sleep 0.2'` → the callback gets "one" and "two" (in any order); `Exited` comes after.
- [ ] Test: `launch_capturing` while active → false.
- [ ] Test: stop works as usual (SIGTERM to the group).

### Task 4: Settings Mirroring page
- [ ] Menu has three rows; Mirroring's detail shows "Low lag" / "Smooth playback".
- [ ] The page has two rows, with the current one checked. Select → `SetMirroringMode(mode)`; selecting the current mode still emits it (idempotent).
- [ ] Back returns to the menu with the Mirroring row focused.

### Task 5: Input gating + `mirroring_ui` + UI
- [ ] Logic test: Back is ignored while app active; Home passes. (This exists implicitly; add an explicit test for Back.)
- [ ] `ui/mirroring.slint` screen; the tile with the cast glyph; MainWindow properties `mirroring-open`, `mirroring-name`, `mirroring-code`; FocusScope routes Escape/Backspace to a `mirroring-back` callback.
- [ ] `mirroring_ui`: open → `launch_capturing(App::Mirroring, "mirroring", command(name, prefs.mode))`; events CodeRequested/Connected/Started/Stopped as in the spec; Back → `runner.stop()`; run exit for `App::Mirroring` → close the screen, toast on quick failure.
- [ ] main: tile indices (apps, then Mirroring, then Settings); `on_action` routes Back to mirroring when its screen is open; Home stops the runner (existing behaviour).
- [ ] Config: `[mirroring] name` (default "mushtv"), with tests in `config`.
- [ ] prefs wiring: load at startup into `Snapshot.mirroring_mode`; `SetMirroringMode` saves, with a toast on failure.
- [ ] Headless: a fake `uxplay` script (on PATH first) that replays the fixture with delays → screenshots of the waiting screen and the code screen, then automatic return to the tiles after "stopped".

### Task 6: Docs + verification + review
- [ ] README: a Mirroring section (packages, the Mac's "Use As Separate Display" tip, the code on first connect, the Settings option, the local-probe note) and checklist items; remove nothing from the roadmap.
- [ ] `cargo test`, clippy, fmt, release build, deploy tests; a fresh reviewer; fix Critical/Important findings test-first.
