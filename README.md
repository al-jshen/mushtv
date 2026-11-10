# mushtv

TV on substrate: the homelab box boots into a full-screen, remote-friendly launcher on the living-room TV. The launcher has tiles for YouTube (VacuumTube), Jellyfin (jellyfin-mpv-shim) and Steam (streamed from the gaming PC with Moonlight). The server workloads keep running underneath.

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
- **Wi-Fi:** for when the box is somewhere without Ethernet, such as a hotel. It needs the one-time switch to iwd below; until then the row says "Not available".
  - The page scans while it's open. The network the box is on comes first, with a check mark, then the rest by signal. A lock marks networks that need a password, and "Saved" marks networks it has joined before.
  - Pick a new network to join it. Open networks connect right away. For a password, a keyboard pops up:
    - **Controller or remote:** arrows move, A/OK presses the key, B/Back closes. X deletes, and Start connects.
    - **Keyboard:** just type. Enter connects, Backspace deletes, Escape closes. You can mix both.
    - Show/Hide reveals what you typed. If the password is wrong, the keyboard comes back with it.
  - Pick the current or a saved network for **Disconnect** or **Connect**, and **Forget**.
  - The box remembers networks it has joined and rejoins them by itself. With Ethernet plugged in, Ethernet stays the main connection.
  - **Not supported:** hotel sign-in pages (the box has no browser: it connects, but nothing loads until you sign in), hidden networks, and enterprise networks (username and password). Hotel Wi-Fi may also stop AirPlay from finding the box.

Needs `bluez` and `libspa-0.2-bluetooth` (installed). Bluetooth logs go to `sudo journalctl -b -t mushtv-launcher`.

**Wi-Fi and Bluetooth share one chip** (Realtek RTL8821CE). On 2.4 GHz Wi-Fi, Bluetooth audio can stutter while the network is busy (YouTube, Moonlight). Use 5 GHz Wi-Fi or Ethernet if that happens.

`launcher/launcher.toml` in the repo is the source of truth. If the installed copy was edited by hand, deploy shows the diff and keeps a `.bak` before replacing it.

### Switching to iwd (once)

Wi-Fi in Settings uses [iwd](https://iwd.wiki.kernel.org/). Before, the box's Wi-Fi was set up by hand in `/etc/network/interfaces`, with wpa_supplicant. The switch moves only the joining of networks to iwd. ifupdown and dhcpcd still give `wlp2s0` its address, with metric 600, so Ethernet stays first. Ethernet, Docker and Tailscale aren't touched.

1. Deploy first (`sudo ./deploy.sh`). It installs the iwd settings: `/etc/iwd/main.conf`, the D-Bus policy that lets the TV use iwd, and a mask that keeps the name `wlp2s0`.
2. With Ethernet plugged in and nothing streaming, see what the switch would do, then do it:
   ```bash
   sudo bash deploy/wifi-switch-to-iwd.sh --dry-run
   sudo bash deploy/wifi-switch-to-iwd.sh
   ```
   It refuses to run without Ethernet. It keeps a copy of `/etc/network/interfaces` as `interfaces.bak-<date>` and moves the home network's password to `/var/lib/iwd/` (root-only). Then it comments out `wlp2s0`'s `wpa-*` lines and starts iwd. Keep the TV out of Settings while it runs.
   - **If `wpa-psk` is a 64-digit key** rather than the password, the script stops. iwd can use a key only with WPA2-only routers. If yours is, add `--hex-ok`. Otherwise put the real password in `wpa-psk` first. Or switch with `--hex-ok` and then join home Wi-Fi again from Settings.
3. Check: `iwctl station wlp2s0 show` says connected, `ip route` still has Ethernet's default route first, and after a reboot the interface is still `wlp2s0`.

**To undo:**
```bash
sudo systemctl mask --now iwd    # stop iwd; masked, so the TV's Settings can't start it again
sudo ifdown wlp2s0
sudo cp -p /etc/network/interfaces.bak-<date> /etc/network/interfaces
sudo systemctl enable --now wpa_supplicant
sudo ifup wlp2s0
```
To go back to iwd later: `sudo systemctl unmask iwd`, then run the switch again.

Logs: `sudo journalctl -u iwd` and the launcher's (`wifi:` lines). A Disconnect from Settings keeps iwd from rejoining any network until you Connect again; that's iwd's own behaviour.

## Jellyfin

The **Jellyfin** tile opens jellyfin-mpv-shim's own library browser, connected to the Jellyfin server on this box (`http://localhost:10000`). The first login needs a keyboard.

- **With a controller:** the D-pad or left stick moves, A opens, B goes back, and X plays or pauses. Start opens the menu for the focused item, or the player's menu during a video. The shoulder buttons page through long rows, and the right stick seeks.
- **Game Controller setting:** the controller works only with Jellyfin's own Game Controller setting turned on, and that setting is off by default. Deploy turns it on (`deploy/jellyfin-gamepad.py`).
  - **Jellyfin must be closed:** it writes its settings back while it runs (on a volume change, and when it closes), which would undo the change. If Jellyfin is open, deploy changes nothing and says so; press Home and deploy again.
  - **First open:** Jellyfin creates its settings file the first time it opens, so deploy again after that.
  - **By hand:** the same switch is in Jellyfin's Settings, under Input. It applies from the next time Jellyfin opens.

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
  - **Graphics card (RTX 4060)** streams from the server `spc-rtx`: full power, but not while someone is playing a game at spc's desk (the card runs out of memory and Moonlight says "No video received from host").
  - **Integrated graphics card** streams from `spc-igpu`: light games (Overcooked 2 and the like) without touching the RTX 4060.
  - Up/Down and OK choose; Back or Home cancels. It starts on the one used last (saved in `~/.local/state/mushtv/settings.toml`, tv's home).
- **Both share one Steam:** one login, games installed once, and the same saves and game settings, so a game continues on either GPU. Only one runs at a time, so picking one GPU ends a session left open on the other (unsaved progress there is lost).
- **Home** (or F12) disconnects but leaves the game running on spc. Opening the tile again with the same GPU picks up where you left off. A session left paused for 30 minutes is ended on spc by itself.
- **To end a session,** choose Power → Exit Steam in Steam. The stream closes and the TV returns to the tiles.
- **Controllers, keyboard and mouse** are passed through to Steam; games see controllers as Xbox controllers (see `server/spc/README.md` for why). Steam's screensaver wakes up only for a controller or the mouse, so turn it off in Steam's settings if you play with a keyboard. The pointer hides after a second without mouse movement. The TV box itself shows a mouse pointer whenever its compositor (cage) thinks a mouse is connected. Two things that aren't mice used to count as one, which left a pointer in the middle of the screen; deploy installs fixes for both, and they take effect when the TV session restarts:
  - **Keyboard interfaces with a scroll axis:** a keyboard's media keys ("… Consumer Control"), and the keyboard part of 8BitDo's 2.4 GHz receiver, count as a keyboard with a scroll wheel. `deploy/libinput-local-overrides.quirks` drops those scroll axes; the keys still work.
  - **8BitDo's 2.4 GHz receiver's mouse:** it never moves. `deploy/8bitdo-mouse.rules` makes the TV ignore it.

  To check what cage thinks is connected, run `sudo -u tv env XDG_RUNTIME_DIR=/run/user/1002 python3 -I - < scripts/seat-capabilities.py` from the repo. It should say "keyboard" only, unless a real mouse is plugged in.
- **Moonlight's own menus** (its settings such as bitrate, Wolf's other apps) have no tile; Moonlight is still installed, so a tile is five lines in `launcher/launcher.toml` if you want them (`command = ["flatpak", "run", "com.moonlight_stream.Moonlight"]`). After a stream ends there, its window can stay black; press Home. Pairing is done from the command line (see `server/spc/README.md`).

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
- **Keyboards that come and go:** an 8BitDo receiver adds a keyboard while its controller is on and removes it when the controller sleeps. That used to leave cage with no keyboard layout to give new apps, and VacuumTube crashed as it started ("YouTube closed unexpectedly"; `sudo journalctl -k | grep segfault` shows `libxkbcommon`). Before each app starts, the launcher now resets cage's keyboard through a virtual keyboard of its own (`launcher/src/seat_keyboard.rs`).

## Development

```bash
cd launcher && cargo test                  # unit tests
scripts/test-seat-keyboard.sh              # the virtual keyboard (above), in a headless cage
scripts/devtools.sh                        # once: fetch grim/wtype/wlr-randr locally (no root)
scripts/headless.sh launcher/launcher.toml /tmp/shots my-scenario.sh [3840x2160]
```

`headless.sh` runs the launcher inside a cage with no display and sources your scenario script. The script can use `shot <name>`, `key <keysym>`, `type_text <text>` and `pause <seconds>`.

With `MUSHTV_DEMO=1`, Settings shows canned Bluetooth, audio and Wi-Fi data and never touches BlueZ, PipeWire or iwd. Use it for screenshots, so they can't disturb the TV's controller or network:
```bash
MUSHTV_DEMO=1 scripts/headless.sh launcher/launcher.toml /tmp/shots launcher/tests/screens/wifi.sh
scripts/test-wifi-switch.sh                # the switch script's parsing, on fixtures
scripts/test-deploy.sh                     # deploy.sh into a temporary folder
```

## Checklist for the real TV

These can't be tested without the TV connected:

- [x] Boot: tty1 logs `tv` in and the launcher appears without a keyboard.
- [ ] Remote: arrows move focus, OK opens, Home returns from every app (`--print-input` should list the remote as `home_source`).
- [ ] Gamepad: D-pad/stick and A work on the home screen. Holding Guide for 1 s returns home from each app, including during a Moonlight stream.
- [ ] If a controller's Guide button shows up as `KEY_HOMEPAGE` in `--print-input`, a short press will go home. Consider removing that key from `home_keys`.
- [x] Each app: opens full-screen, has audio over HDMI, uses hardware decode (VacuumTube: `--debug-gpu`), and Home closes it within about 3 s.
- [x] jellyfin-mpv-shim: first login (keyboard needed once), server `http://localhost:10000`.
- [ ] Jellyfin with a controller (after deploying with Jellyfin closed): the D-pad moves around the library, A plays a video, B goes back.
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
- [ ] No pointer on the tiles or over a Steam stream, with the keyboard plugged in and the 8BitDo controller on (after `sudo ./deploy.sh --restart`).
- [ ] YouTube opens after the 8BitDo Ultimate 2C was turned on and then off again (it disconnects after a while idle), without restarting the TV session.
- [ ] Wi-Fi switch: after `deploy/wifi-switch-to-iwd.sh`, Settings → Wi-Fi lists networks, home Wi-Fi is connected, and Ethernet keeps the default route.
- [ ] Wi-Fi: join a phone hotspot with only a controller, then again (Forget first) with only a keyboard.
- [ ] Wi-Fi: a wrong password brings the keyboard back with the text kept, and a toast.
- [ ] Wi-Fi: after a reboot, `wlp2s0` keeps its name and rejoins home Wi-Fi by itself.

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
