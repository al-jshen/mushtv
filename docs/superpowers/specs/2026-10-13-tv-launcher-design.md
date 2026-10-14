# mushtv launcher: design

Date: 2026-10-13. Context: `handoff.md` (box, OS, goals). Decisions below were agreed in conversation.

## Goal

A full-screen, couch-friendly ("10-foot") launcher for the living-room TV, running under `cage` as the unprivileged `tv` user. It shows app tiles, starts the chosen app, and comes back when the app exits. A global Home gesture gets back from any app. It must stay light and never disturb the server workloads (Docker, SSH).

Success criteria:

- After power-on, the TV shows the launcher with no keyboard.
- Any tile can be opened with a remote or a gamepad.
- Home returns to the launcher within about 1 s; after a crash, the launcher is back within a few seconds.
- While idle, the launcher uses negligible CPU and well under 100 MB RAM.

## Scope

In v1:

- One row of tiles from a config file, a clock with date, focus animation, a nice background.
- Keyboard/remote, air-mouse pointer, and gamepad navigation.
- Global Home: configurable key(s), plus holding a gamepad's Guide button.
- Launch/stop/return lifecycle; error messages on screen; logs in the journal.
- `deploy.sh` that installs everything, including the boot-to-kiosk setup.

Not in v1 (later): Jellyfin backdrop slideshow ("top shelf"), display blanking/screensaver, CEC, background cast target for jellyfin-mpv-shim, refresh-rate matching, folders, on-screen settings.

## Verified facts this design relies on

- **cage hands the screen back.** Probe: with two windows in headless cage, opening B hides A (A gets `wl_surface.leave` and loses `activated`). Killing B shows A again and re-activates it within milliseconds. While hidden, A gets no frame callbacks, so a hidden launcher does not render.
- **Flatpak process tree.** `flatpak run` execs into `bwrap`; the app runs two levels down **in the same process group**. Signalling only the direct child leaves the app running. Signalling the process group stops everything, and the sandbox's `xdg-dbus-proxy` exits by itself.
- **Flatpak video decode.** The Flatpak runtime's Intel driver (iHD 26.3.5, `org.freedesktop.Platform.VAAPI.Intel`) hardware-decodes AV1 inside the sandbox.
- **Installed apps** (system-wide): `com.moonlight_stream.Moonlight` 6.2.0, `rocks.shy.VacuumTube` 2.0.0, `com.github.iwalton3.jellyfin-mpv-shim` 3.1.0. Exported icons live in `/var/lib/flatpak/exports/share/icons/hicolor/`.
- **Jellyfin** is on host port 10000 (`http://localhost:10000`), not 8096.
- **Toolchain:** Rust 1.99 via rustup in `~mushy/.cargo`. No `pkg-config` or `-dev` packages installed, so dependencies must not need them at build time.

## Technology

- Rust + Slint 1.18 (winit Wayland backend, FemtoVG OpenGL ES renderer). UI in `.slint` markup, logic in Rust.
- Input via the `evdev` crate directly, for both gamepads and the Home key. `gilrs` was the original plan, but it needs `libudev-dev` and `pkg-config` to build. Mainstream controllers (xpad, hid-playstation, hid-nintendo, 8BitDo in X-input mode) already report the kernel's standard gamepad codes (`BTN_SOUTH`, `BTN_MODE`, `ABS_HAT0X`, …), which is all we need.
- Config: TOML (`serde` + `toml`). Logging: `tracing` → journald (`tracing-journald`), with stderr as a fallback.

## Architecture

Single binary `mushtv-launcher`, source in `launcher/`.

| Module | Responsibility | Interface |
|---|---|---|
| `config` | Load and validate TOML; resolve tile icons | `Config::load(path) -> Result<Config>` |
| `ui/*.slint` | Home screen: background, tiles, focus, clock, overlays | properties (`tiles`, `focused`, `busy-text`, `toast`, `error-screen`) and callbacks (`activate(i)`) |
| `runner` | Spawn the app in its own process group; stop it with escalation; report exit | `Runner::launch(tile)`, `Runner::stop()`, exit events |
| `input` | Read `/dev/input`; gamepad navigation; Home detection; hotplug | emits `InputAction::{Left, Right, Activate, Home}` |
| `input::logic` | Pure state machine: Guide hold timing, debounce, cooldown, stick/D-pad repeat | `on_event(ev, now) -> Vec<InputAction>`, `next_deadline()` |
| `main` | Wires the modules together; owns the "which app is running" state | — |

Threads: the Slint event loop (main thread); one thread per input device doing blocking reads; an inotify thread on `/dev/input` for hotplug; one input-logic thread (channel in, `recv_timeout` for hold and repeat deadlines); one waiter thread per launched app. Everything that changes the UI goes through `slint::invoke_from_event_loop`.

## Input

Keyboard and remote, through cage/Wayland, so they only arrive while the launcher is in front:

- Left/Right move focus. Up/Down do nothing in v1.
- Enter, Return or keypad Enter open the focused tile. A remote's OK/Select key (`KEY_OK`, `KEY_SELECT`) has no keysym, so Wayland never delivers it; the launcher reads it from evdev instead, gated exactly like the gamepad's A button.
- Escape/Back do nothing on the home screen.
- Pointer: hover focuses, click opens.

Gamepads, read from evdev, so they are seen even while an app is in front:

- While the launcher is in front: D-pad (`ABS_HAT0X/Y` or `BTN_DPAD_*`) and the left stick (deadzone 50 % of range) move focus. Holding repeats after 400 ms, then every 150 ms. `BTN_SOUTH` (A) opens the tile.
- While an app is running: everything is ignored except a Guide hold.
- Devices are never grabbed (`EVIOCGRAB`), so apps still receive all input.

Home, at all times:

- Any key listed in `home_keys` (default `["KEY_HOMEPAGE"]`), from any keyboard-like device. The keyboard's Home key (`KEY_HOME`) is excluded by default because apps use it in text fields.
- `BTN_MODE` (Guide) held for `guide_hold_ms` (default 1000) on any gamepad. A short press is ignored.
- After a Home action, further Home actions are ignored for 1 s.
- When the launcher comes back to the front, all input is ignored for 500 ms, so a held button or repeating key can't immediately reopen the app.
- Pressing Home on the home screen does nothing.

## App lifecycle

States: `Idle → Opening → Running → Stopping → Idle`.

1. **Open.**
   - Spawn `command` with its own process group (`setpgid(0,0)`) and `PR_SET_PDEATHSIG = SIGTERM`. For `flatpak run` commands the launcher also adds `--die-with-parent`, because the parent-death signal only reaches the outer `bwrap` and would leave the app running. Together they make a dying launcher take its app with it.
   - Connect the app's stdout/stderr to the journal with identifier `mushtv-<tile-id>`.
   - Show the "Opening <name>…" overlay. Ignore further activations while not `Idle`.
2. **Running.** The app window covers the launcher. The launcher idles, listening only for Home.
3. **Stop** (Home, or launcher shutdown, including when the UI loop ends with an error because cage went away first). Escalate:
   1. Run `stop_command` if set, and wait 3 s.
   2. `SIGTERM` the process group, and wait 3 s.
   3. `SIGKILL` the process group. For Flatpaks this kills the sandbox's PID-namespace init, so the kernel removes everything inside.
4. **Exit.** When the app exits by any route, go back to `Idle` and clear the overlay. If it exited non-zero within 10 s of starting, show the toast "<name> closed unexpectedly". If spawning failed, show "Couldn't open <name>". Details go to the journal.

On `SIGTERM`/`SIGINT` the launcher stops the running app, using the same sequence, before exiting.

## Config

The source of truth is `launcher/launcher.toml` in the repo. `deploy.sh` installs it to `/etc/mushtv/launcher.toml`. The launcher reads `--config <path>` if given, else `/etc/mushtv/launcher.toml`. `--check-config <path>` validates and exits.

```toml
[launcher]
clock = "24h"                 # "24h" | "12h"
home_keys = ["KEY_HOMEPAGE"]  # evdev key names
guide_hold_ms = 1000

[[tile]]
id = "youtube"                # used in the journal identifier; [a-z0-9-]+, unique
name = "YouTube"
command = ["flatpak", "run", "rocks.shy.VacuumTube", "--fullscreen"]
# icon = "youtube.png"        # optional; relative paths resolve against /usr/local/share/mushtv/
# color = "#c4302b"           # optional tile background
# stop_command = [...]        # optional
```

- **Icon resolution.** An explicit `icon` wins. Otherwise, if `command` is `flatpak run [opts] <app-id>`, use the largest exported icon for `<app-id>` (SVG preferred, then the biggest PNG). Otherwise draw the name as text.
- **Validation.** At least one tile; non-empty `command`; unique valid `id`s; known `home_keys` names; `guide_hold_ms` between 300 and 5000.
- **Bad config.** Show a full-screen error with the message and path; it doesn't crash. Home and the input watcher still work.

## Visual design

The canvas is designed at 1920×1080 logical units and scaled by `window height / 1080`, so 4K renders at 2×. cage uses output scale 1, so physical pixels equal logical pixels.

- **Background.** A dark vertical gradient (`#0d1117` → `#1b2230`) with a soft radial highlight upper-left. If `/usr/local/share/mushtv/background.(png|jpg)` exists, it is used instead, with a dark overlay.
- **Clock.** Top-right, 48 px margin: time 56 px light weight; below it the date, 24 px at 70 % opacity ("Tue 13 Oct"). Updates once per second.
- **Tile row.** Vertically centered slightly below middle (center at 56 % of height). Tiles are 400×240 with corner radius 28 and 56 px spacing. The row is centered when it fits; otherwise it scrolls horizontally (animated) to keep the focused tile visible.
- **Tile content.** A background colour (config `color`, else a default from a small palette by index) with the icon centered at 55 % of the tile's height, or the name in 40 px semibold if there is no icon.
- **Focus.**
  - The focused tile grows to 1.12× (180 ms ease-out) and gets a 4 px white border at 90 % opacity and a soft white glow (drop shadow, blur 48, 35 % white).
  - Unfocused tiles sit at 70 % opacity.
  - The focused tile's name appears centered below the row (32 px, fades in 150 ms).
- **Overlays.**
  - "Opening <name>…": the whole screen dims to 60 % black, with a centered label and a rotating arc spinner.
  - Toast: a bottom-center pill (dark red, 28 px text) that auto-hides after 5 s.
  - Config error: a full-screen panel with a title and monospace details.
- **Font.** Inter (OFL), embedded in the binary from Debian's `fonts-inter`, with `LICENSE` alongside. Fallback: DejaVu Sans.

## Deployment

You build as `mushy`, without sudo (`cargo build --release` in `launcher/`). `sudo ./deploy.sh [--restart]` installs:

| What | Where |
|---|---|
| binary | `/usr/local/bin/mushtv-launcher` (via `install`, so a running binary is never overwritten in place) |
| assets (font licence, optional background, fallback icons) | `/usr/local/share/mushtv/` |
| config | `/etc/mushtv/launcher.toml`. Validated with `--check-config` first; if the installed copy differs, the script shows a diff and keeps `launcher.toml.bak`. |
| autologin | `/etc/systemd/system/getty@tty1.service.d/autologin.conf`, then `systemctl daemon-reload` |
| tv login script | `/home/tv/.bash_profile` (owner `tv:tv`) |

Everything except `.bash_profile` is root-owned and only readable by `tv`. The script is idempotent. `--restart` runs `systemctl restart getty@tty1`, which restarts the TV session (and closes whatever is playing).

`.bash_profile`:

```bash
if [ "$(tty)" = "/dev/tty1" ]; then
  cage -s -- /usr/local/bin/mushtv-launcher
  sleep 2   # avoid a tight restart loop if the launcher dies at startup
  exit
fi
```

Boot chain: systemd → getty@tty1 (autologin `tv`) → bash login → cage → launcher. If the launcher exits for any reason, the session ends and getty logs `tv` in again.

Logs: `sudo journalctl -b -t mushtv-launcher` (launcher) and `-t mushtv-<tile-id>` (apps). Debugging: SSH in, or Ctrl+Alt+F2 for a text console.

## Testing

- **Unit tests** (`cargo test`):
  - config parsing, validation and icon resolution;
  - the input logic state machine with injected time (hold threshold, short press ignored, debounce, cooldown, stick deadzone and repeat, gating while an app runs);
  - the runner against real processes: a process-group kill reaches a grandchild; escalation to SIGKILL when SIGTERM is trapped; `stop_command` path; exit-code reporting.
- **Headless integration:**
  - run under `cage` with `WLR_BACKENDS=headless`;
  - take screenshots with `grim`, extracted from Debian's `.deb` into a dev-only directory (no install);
  - drive keys with a virtual keyboard (`wtype`, same approach) if cage accepts it; otherwise use a debug-only test hook;
  - check that the home screen renders, a test tile launches and returns, and errors show.
- **On the real TV (manual):** real remote and gamepad, Guide hold during Moonlight, hardware decode in each app, 4K rendering smoothness.
