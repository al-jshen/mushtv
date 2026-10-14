# Mirroring (AirPlay screen mirroring from a Mac): design

Date: 2026-10-14. This extends the launcher and Settings specs. Decisions were agreed in conversation; the user approved the sections and asked for implementation.

## Goal

From the couch, open **Mirroring** on the TV and mirror a MacBook's screen to it with AirPlay. The receiver exists only while the Mirroring screen is open. A Mac connecting for the first time must enter a code shown on the TV. Responsiveness is the default; smooth video playback is a Settings option.

## Verified facts (spike with the user's MacBook, this box)

- **Packages:** UxPlay 1.71.1 (Debian) works. `avahi-daemon` advertises it, and the Mac found "mushtv" over Wi-Fi despite ~12 Docker bridge interfaces.
- **Decoding:** in the TV session (`tv` is in `render`), `-vd vah264dec -vc vapostproc` decodes on the GPU. A GStreamer debug run showed `waylandsink` negotiating `video/x-raw(memory:DMABuf)` 1920×1080, with UxPlay's `videoscale` in passthrough. Frames stay on the GPU all the way to the screen.
- **CPU:** ~50% of one core while mirroring at 30 fps (network, decryption, audio). ~0% while idle and waiting.
- **Sharpness:** best with the Mac set to "Use As Separate Display" (the TV panel is 1080p). Mirroring the 16:10 built-in display is softer.
- **Latency:** `-vsync no` (no timestamp sync) noticeably lowers it; audio stayed in sync in the user's test. With it, video playback hitches a little; the box isn't CPU-bound, so the cause is likely network jitter.
- **Code request:** `-pin` makes UxPlay ask the Mac for a random 4-digit code on first pairing. After that the Mac is trusted and isn't asked again.
- **UxPlay's output** (captured from a real session; fixture `launcher/tests/fixtures/uxplay-session.log`):
  - `*** CLIENT MUST NOW ENTER PIN = "7932" AS AIRPLAY PASSWORD`
  - `connection request from <name> (<model>) with deviceID = <id>`
  - `raop_rtp_mirror starting mirroring`
  - `raop_rtp_mirror->running is no longer true` (mirroring stopped)
  - "Connection closed for socket N" also appears during the code exchange, so it is **not** an end signal.
- **Local probes:** when the receiver appears, a host-network service on this box (Home Assistant or Music Assistant) connects and probes it. That produces only "Accepted…/ignoring AirPlay video streaming request/Connection closed" lines and must be ignored.
- **Buffering:** UxPlay's stdout is block-buffered when piped, so the launcher runs it under `stdbuf -oL -eL` to get lines as they're printed. The capture used this.

## Screens

- **Home row:** a built-in **Mirroring** tile (Material Icons "cast" glyph) between the configured apps and Settings.
- **Mirroring screen** (same background and clock):
  - title "Mirroring";
  - "On your Mac: Control Center → Screen Mirroring → <name>";
  - footer "Press Back to stop".
  - When the Mac asks for a code: "Enter this code on your Mac" with the 4 digits large. The code clears once the Mac connects.
- **Flow:**
  - Opening the tile shows the screen and starts UxPlay.
  - Mirroring covers the screen with the Mac's picture.
  - When mirroring stops, from the Mac or by disconnecting, UxPlay is stopped and the tiles return.
  - Home, from the screen or during mirroring, stops it and returns to the tiles.
  - Back stops it from the screen. During mirroring, only Home works.
- **Failure:** if UxPlay is missing or exits with an error within 10 s, the toast "Couldn't start mirroring" appears and the tiles stay. Details go to the journal.
- **Settings:** a third menu row, "Mirroring" (detail: the current mode). Its page has "Low lag" (default) and "Smooth playback", with a check mark on the current one. A change applies the next time Mirroring opens.

## Architecture

| Unit | Responsibility | Interface |
|---|---|---|
| `mirroring` (pure) | UxPlay command for a name and mode; parse output lines into events | `MirrorMode { LowLag, Smooth }`, `command(name, mode) -> Vec<String>`, `MirrorEvent { CodeRequested(String), Connected(String), Started, Stopped }`, `parse_line(&str) -> Option<MirrorEvent>` |
| `prefs` | The launcher's own settings, saved atomically to `$XDG_STATE_HOME/mushtv/settings.toml` (default `~/.local/state/mushtv/settings.toml`) | `Prefs { mirroring_mode }`, `load(path) -> Prefs` (missing or corrupt → defaults, logged), `save(path, &Prefs) -> Result<()>`, `default_path()` |
| `runner` (extended) | Launch with output capture: stdout and stderr are piped and each line goes to a callback and to the journal; process group, stop escalation, pdeathsig and exit tracking are unchanged | `App { Tile(usize), Mirroring }` in `RunEvent`; `launch_capturing(app, id, &command, on_line) -> bool` |
| `settings` (extended) | Mirroring page; `Effect::SetMirroringMode(MirrorMode)`; `Snapshot.mirroring_mode` | as existing |
| `mirroring_ui` (UI thread) | Opens and closes the screen, starts UxPlay through the runner, reacts to parsed events, handles Back | `open()`, `is_open()`, `back()`, `on_run_exit()` |
| `ui/mirroring.slint` | The Mirroring screen | properties `mirroring-name`, `mirroring-code` |

**UxPlay command:**
```
stdbuf -oL -eL uxplay -n <name> -nh -p -fs -vs waylandsink -vd vah264dec -vc vapostproc -s 1920x1080@30 -fps 30 -pin
```
plus `-vsync no` in low-lag mode. `<name>` comes from the config: `[mirroring] name` (default "mushtv"). The runner adds nothing: this isn't a `flatpak run` command.

**Input:**
- While the Mirroring screen waits, the launcher has focus, so the input thread's app gate stays open. Back (Escape or Backspace via Slint; gamepad B or remote Back via evdev) and Home stop it.
- On `Started`, the gate closes (`set_app_active(true)`), so only Home passes.
- When UxPlay exits, the return cooldown applies as for any app.

## Errors

- **Unknown output lines** are logged at debug level and ignored. If a UxPlay update changes its wording:
  - a missed code request means no code is shown; Home still works;
  - a missed "stopped" means UxPlay keeps running until Home or Back.
- **Settings file:** if it's missing or corrupt, defaults are used and a warning logged. If saving fails, the toast "Couldn't save the setting" appears and the choice still applies for this session.
- **Launching while something else runs:** impossible from the UI. The runner refuses as before.

## Testing

- **Unit:**
  - `command` for both modes;
  - `parse_line` over the captured fixture, which yields exactly [CodeRequested("7932"), Connected("Living Room Mac"), Started, Stopped]; local-probe lines and junk give nothing;
  - prefs: round trip, missing, corrupt, unwritable;
  - `launch_capturing` with a real process printing to stdout and stderr, then exit;
  - Settings Mirroring page;
  - input gating (Back while waiting; only Home while mirroring).
- **Headless screenshots:** the Mirroring screen with and without a code (driven by a fake UxPlay script that prints the captured lines), and the Settings page.
- **With the Mac, on the TV:** first-time code, reconnect without a code, stop from the Mac returns to the tiles, Home during mirroring, both modes.
