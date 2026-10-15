# mushtv

TV on substrate: the homelab box boots into a full-screen, remote-friendly launcher on the living-room TV. The launcher has tiles for YouTube (VacuumTube), Jellyfin (jellyfin-mpv-shim), Steam (streamed from the gaming PC) and Moonlight. The server workloads keep running underneath.

- Background and decisions: `handoff.md`, `docs/superpowers/specs/2026-10-13-tv-launcher-design.md`
- Launcher source: `launcher/` (Rust + Slint)

## How it fits together

```
systemd → getty@tty1 (autologin tv) → ~tv/.bash_profile → cage → mushtv-launcher → flatpak run <app>
```

- **cage** shows the newest window full-screen. When an app exits, the launcher is back on screen.
- **Home:**
  - The Home key on a remote (`KEY_HOMEPAGE`), F12 on a keyboard, or holding a gamepad's Guide button for 1 s closes the running app.
  - The launcher reads these straight from `/dev/input`, so they work whichever app is in front.
- **If the launcher dies,** the login session ends and getty starts it again.

## Build and deploy

```bash
cd launcher && cargo build --release && cd ..
sudo ./deploy.sh              # installs; the running launcher keeps the old version
sudo ./deploy.sh --restart    # ...and restarts the TV session (closes what's playing)
```

The first deploy sets up autologin. Run it with `--restart`, or reboot, to switch tty1 from a login prompt to the launcher.

| What | Installed to |
|---|---|
| launcher | `/usr/local/bin/mushtv-launcher` |
| config (from `launcher/launcher.toml`) | `/etc/mushtv/launcher.toml` |
| tile icons, `background.png`/`.jpg` (from `launcher/assets/share/`) | `/usr/local/share/mushtv/` |
| autologin | `/etc/systemd/system/getty@tty1.service.d/autologin.conf` |
| tv's login script (from `deploy/bash_profile`) | `/home/tv/.bash_profile` |
| HDMI audio rule (from `deploy/wireplumber-hdmi.conf`) | `/home/tv/.config/wireplumber/wireplumber.conf.d/51-mushtv-hdmi.conf` |

The onboard sound card exposes analog and HDMI as separate profiles, and PipeWire picks analog (the headphone jack) by default. The audio rule makes the TV session prefer HDMI. It also ranks Bluetooth outputs below HDMI, so a speaker never takes over until you choose it. When the rule changes, deploy restarts tv's WirePlumber, so audio moves over straight away.

## Settings

The gear tile at the end of the row opens Settings (Up/Down, OK, Back; Home returns to the tiles):

- **Audio output:** choose the TV or a paired Bluetooth speaker or headphones. The choice is remembered.
  - A chosen speaker gets the audio whenever it's connected, and audio falls back to the TV when it isn't.
  - Picking a speaker that's off connects it first.
- **Bluetooth:** paired devices (connect, disconnect, forget) and **Add device**.
  - Add device scans while it's open. Pick a speaker, controller or keyboard in pairing mode.
  - Speakers and controllers pair without a code. Keyboards show a code to type.
  - Nearby devices may include neighbours' gadgets; speakers, controllers, keyboards and mice are listed first.

Needs `bluez` and `libspa-0.2-bluetooth` (installed). Bluetooth logs go to `sudo journalctl -b -t mushtv-launcher`.

**Wi-Fi and Bluetooth share one chip** (Realtek RTL8821CE). On 2.4 GHz Wi-Fi, Bluetooth audio can stutter while the network is busy (YouTube, Moonlight). Use 5 GHz Wi-Fi or Ethernet if that happens.

`launcher/launcher.toml` in the repo is the source of truth. If the installed copy was edited by hand, deploy shows the diff and keeps a `.bak` before replacing it.

## Mirroring

The **Mirroring** tile turns the TV into an AirPlay screen for a Mac, but only while that screen is open. Nothing on the network can see it otherwise.

- **To connect:** on the Mac, Control Center → Screen Mirroring → **mushtv**. Change the name with `[mirroring] name` in `launcher/launcher.toml`.
- **The first time a Mac connects,** the TV shows a 4-digit code to type on the Mac. After that, that Mac connects without a code.
- **For the sharpest picture,** choose **Use As Separate Display** on the Mac instead of mirroring its built-in screen. The TV is 1080p; mirroring squeezes the MacBook's 16:10 Retina screen and softens text.
- **Stopping:** stop mirroring on the Mac, or press Home (or Back on the waiting screen). Both return to the tiles and close the receiver.
- **Settings → Mirroring:** **Low lag** (default) is responsive but video can hitch a little. **Smooth playback** syncs audio and video at the cost of about half a second of lag. The choice is saved in `~/.local/state/mushtv/settings.toml` (tv's home) and applies the next time Mirroring opens.

It runs [UxPlay](https://github.com/FDH2/UxPlay) with GPU decoding (frames stay on the GPU all the way to the screen). It needs `uxplay`, `avahi-daemon` and `gstreamer1.0-plugins-bad` (installed). Its log lines appear in `sudo journalctl -t mushtv-launcher` prefixed with `mirroring:`.

When the receiver starts, Home Assistant or Music Assistant on this box may briefly probe it (they look for AirPlay devices). That's harmless and ignored.

## Games (Steam)

The **Steam** tile streams Steam's TV interface (Big Picture) from the gaming PC, spc, with Moonlight, at 1080p and 60 fps over Tailscale. On spc, [Wolf](server/spc/README.md) gives the TV its own invisible desktop, so someone can keep using spc's own screen at the same time.

- **Opening the tile asks which GPU:**
  - **Graphics card (RTX 4060)** streams from the server `spc`: full power, but not while someone is playing a game at spc's desk (the card runs out of memory and Moonlight says "No video received from host").
  - **Integrated graphics card** streams from `spc-igpu`: light games (Overcooked 2 and the like) without touching the RTX 4060.
  - Up/Down and OK choose; Back or Home cancels. It starts on the one used last (saved in `~/.local/state/mushtv/settings.toml`, tv's home).
- **Both share one Steam:** one login, games installed once. Only one runs at a time, so picking one GPU ends a session left open on the other (unsaved progress there is lost).
- **Home** (or F12) disconnects but leaves the game running on spc. Opening the tile again with the same GPU picks up where you left off. A session left paused for 30 minutes is ended on spc by itself.
- **To end a session,** choose Power → Exit Steam in Steam. The stream closes and the TV returns to the tiles.
- **Controllers, keyboard and mouse** are passed through to Steam; games see controllers as Xbox controllers (see `server/spc/README.md` for why). Steam's screensaver wakes up only for a controller or the mouse, so turn it off in Steam's settings if you play with a keyboard.
- **The Moonlight tile** opens Moonlight's own menus, which are useful for pairing or other hosts. After a stream ends there, its window can stay black; press Home.

Setup, the GPU choice and limits are in [`server/spc/README.md`](server/spc/README.md). Moonlight's log is `sudo journalctl -t mushtv-steam`.

## Adding a tile

Add a `[[tile]]` to `launcher/launcher.toml`. The comments in that file list every field. Then check it, build and deploy:

```bash
launcher/target/release/mushtv-launcher --check-config launcher/launcher.toml
sudo ./deploy.sh --restart
```

Flatpak apps get their own icon automatically. For anything else, put an image in `launcher/assets/share/` and set `icon = "file.png"`.

## Logs and debugging

```bash
sudo journalctl -b -t mushtv-launcher        # the launcher
sudo journalctl -b -t mushtv-youtube         # an app's output: mushtv-<tile id>
sudo journalctl -b -u getty@tty1             # the autologin session
sudo -u tv /usr/local/bin/mushtv-launcher --print-input   # which remotes/gamepads it sees
```

- **Text console:** press Ctrl+Alt+F2, or SSH in.
- **Back to a normal login prompt on tty1:** delete the autologin override, then `sudo systemctl daemon-reload && sudo systemctl restart getty@tty1`.

## Development

```bash
cd launcher && cargo test                  # unit tests
scripts/devtools.sh                        # once: fetch grim/wtype/wlr-randr locally (no root)
scripts/headless.sh launcher/launcher.toml /tmp/shots my-scenario.sh [3840x2160]
```

`headless.sh` runs the launcher inside a cage with no display and sources your scenario script. The script can use `shot <name>`, `key <keysym>` and `pause <seconds>`.

## Checklist for the real TV

These can't be tested without the TV connected:

- [x] Boot: tty1 logs `tv` in and the launcher appears without a keyboard.
- [ ] Remote: arrows move focus, OK opens, Home returns from every app (`--print-input` should list the remote as `home_source`).
- [ ] Gamepad: D-pad/stick and A work on the home screen. Holding Guide for 1 s returns home from each app, including during a Moonlight stream.
- [ ] If a controller's Guide button shows up as `KEY_HOMEPAGE` in `--print-input`, a short press will go home. Consider removing that key from `home_keys`.
- [x] Each app: opens full-screen, has audio over HDMI, uses hardware decode (VacuumTube: `--debug-gpu`), and Home closes it within about 3 s.
- [x] jellyfin-mpv-shim: first login (keyboard needed once), server `http://localhost:10000`.
- [x] The launcher looks right and animates smoothly at the TV's real resolution (4K?).
- [x] Crash recovery: `sudo pkill -f mushtv-launcher` brings the launcher back within about 5 s.
- [x] Docker and SSH stay responsive while the TV is in use.
- [x] Settings → Bluetooth → Add device: pair a speaker and a controller; both show as paired and connected.
- [ ] Settings → Audio output: switch to the speaker and back to TV while something plays; audio moves without restarting the app.
- [ ] With the speaker chosen: turn it off (audio falls back to the TV), then on (audio returns to the speaker).
- [ ] With the TV chosen: turn the speaker on; audio stays on the TV.
- [ ] Mirroring: a new Mac is asked for the code shown on the TV; once entered, the Mac's picture fills the TV.
- [ ] Mirroring again from the same Mac: no code asked.
- [ ] Stopping mirroring on the Mac returns the TV to the tiles; so does Home during mirroring, and Back on the waiting screen.
- [ ] Settings → Mirroring → Smooth playback: the next session plays video more smoothly, with more lag; Low lag returns to responsive.
- [ ] Steam tile: the GPU picker appears and starts on the GPU used last; Back returns to the tiles.
- [ ] Steam tile: Steam's TV interface appears on both GPUs; keyboard, mouse and a controller all work in it.
- [ ] Steam tile: switching GPUs ends the session on the other one, and Steam starts (with the same login) within a minute.
- [ ] Steam tile: Home (and F12) returns to the tiles; opening the tile again resumes the same game.
- [ ] Steam tile: Power → Exit Steam returns the TV to the tiles.

## Roadmap

Ideas for later, not yet scheduled:

- **Two Bluetooth speakers as one output (stereo pair or party mode).**
  - **How:** PipeWire 1.4.2's `libpipewire-module-combine-stream` can feed one virtual output to several speakers. It can map channels per speaker (left to one, right to the other, or the full mix to both) and compensate for each speaker's latency (`combine.latency-compensate`). It would show up in Settings → Audio output as e.g. "Speaker pair".
  - **Caveats:**
    - Speakers buffer internally by amounts they don't report, so they may drift some milliseconds apart (blurred stereo, or an echo when both play the full mix).
    - Both streams share the one radio, which is the same chip as the Wi-Fi.
    - The Bluetooth 4.2 radio can't do Auracast/LE Audio.
  - **First check** whether the speakers have their own stereo pairing (JBL PartyBoost, UE PartyUp, Sony, Bose…). If they do, link them to each other and connect just one; that already works and syncs better.
  - **Plan:** a config-only experiment with two real speakers, listen for sync and dropouts, then a Settings option if it's good enough.
- **Jellyfin backdrops on the home screen** (an Apple TV-style "top shelf").
- **Display blanking when idle**, and TV power on/off over HDMI-CEC (needs a USB-CEC adapter).
- **jellyfin-mpv-shim as an always-on cast target**, so phones can cast to the TV without opening the Jellyfin tile.
- **Refresh-rate matching** for video (e.g. 24 Hz for films).
