# mushtv

TV on substrate: the homelab box boots into a full-screen, remote-friendly launcher on the living-room TV. The launcher has tiles for YouTube (VacuumTube), Jellyfin (jellyfin-mpv-shim) and Moonlight. The server workloads keep running underneath.

- Background and decisions: `handoff.md`, `docs/superpowers/specs/2026-10-13-tv-launcher-design.md`
- Launcher source: `launcher/` (Rust + Slint)

## How it fits together

```
systemd → getty@tty1 (autologin tv) → ~tv/.bash_profile → cage → mushtv-launcher → flatpak run <app>
```

- **cage** shows the newest window full-screen. When an app exits, the launcher is back on screen.
- **Home:**
  - The Home key on a remote (`KEY_HOMEPAGE` by default), or holding a gamepad's Guide button for 1 s, closes the running app.
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

`launcher/launcher.toml` in the repo is the source of truth. If the installed copy was edited by hand, deploy shows the diff and keeps a `.bak` before replacing it.

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

- [ ] Boot: tty1 logs `tv` in and the launcher appears without a keyboard.
- [ ] Remote: arrows move focus, OK opens, Home returns from every app (`--print-input` should list the remote as `home_source`).
- [ ] Gamepad: D-pad/stick and A work on the home screen. Holding Guide for 1 s returns home from each app, including during a Moonlight stream.
- [ ] If a controller's Guide button shows up as `KEY_HOMEPAGE` in `--print-input`, a short press will go home. Consider removing that key from `home_keys`.
- [ ] Each app: opens full-screen, has audio over HDMI, uses hardware decode (VacuumTube: `--debug-gpu`), and Home closes it within about 3 s.
- [ ] jellyfin-mpv-shim: first login (keyboard needed once), server `http://localhost:10000`.
- [ ] The launcher looks right and animates smoothly at the TV's real resolution (4K?).
- [ ] Crash recovery: `sudo pkill -f mushtv-launcher` brings the launcher back within about 5 s.
- [ ] Docker and SSH stay responsive while the TV is in use.
