# Steam on either GPU (two Wolf servers, a GPU picker, idle sessions ended): design

Date: 2026-10-14. This extends the launcher spec and the Wolf setup in `server/spc/`. Decisions were agreed in conversation; the user approved the design and asked for a spec.

## Goal

From the TV, start Steam on spc's **RTX 4060** (full power, when nobody is gaming at spc's desk) or on its **integrated graphics** (light games such as Overcooked 2, without touching the RTX 4060 that the desk uses). Both run all the time; the Steam tile asks which to use. Both share one Steam: one login, games installed once. A session left paused (Home, or the TV switched off) ends by itself after 30 minutes. Nothing in daily use needs SSH.

## Verified facts

- **The RTX 4060 can't be shared with a game at the desk.** One desk game held 5.8 GB of its 8 GB; Wolf's encoder then failed with `CUDA_ERROR_OUT_OF_MEMORY` and Moonlight showed "No video received from host". NVIDIA's Linux driver doesn't spill to system RAM.
- **The integrated graphics works for Wolf** (`WOLF_RENDER_NODE` set to the `amdgpu` render node): H.264 and H.265 on the VA hardware encoder, zero-copy; Steam's TV interface streamed at 1080p60 to a test client while the desk game kept its memory. AV1 there is software-only (`aom`), but Moonlight's Automatic codec prefers H.265 on the TV, which decodes it in hardware.
- **Ports:** Wolf reads them from `WOLF_HTTPS_PORT`, `WOLF_HTTP_PORT`, `WOLF_CONTROL_PORT`, `WOLF_RTSP_SETUP_PORT`, `WOLF_VIDEO_PING_PORT` and `WOLF_AUDIO_PING_PORT` (`state/data-structures.hpp`). Moonlight takes `host:port` for a manually added server (`ComputerManager::addNewHostManually` parses it with `QUrl`); the other ports come from the server during setup of each stream.
- **Per-device app folders:** an app's home is `<state folder>/<hash of the device's Moonlight certificate>/<app title>` (`rest/endpoints.hpp`). Moonlight uses one certificate for every server, so two servers with the same state folder give the TV the same Steam home. The TV's folder is `/etc/wolf/5334278758390175681/Steam`.
- **Session IDs** are the same hash, so the TV's session has the same ID on both servers (the Steam container is named `WolfSteam_<session id>`).
- **Wolf's API** (Unix socket `wolf.sock` in its runtime folder): `GET /api/v1/events` streams every internal event as Server-Sent Events, with the type name as the event and its JSON as data. `PauseStreamEvent`, `ResumeStreamEvent` and `StopStreamEvent` carry `session_id`. A new session fires `StreamSession` (it has `session_id` too) when Moonlight launches an app (`rest/endpoints.hpp`); reconnecting to a paused session takes the resume path instead. The exact JSON of `StreamSession` is checked on spc before relying on it. `GET /api/v1/sessions` lists sessions; `POST /api/v1/sessions/stop` with `{"session_id": "…"}` ends one.
- **Pausing:** a Moonlight disconnect (Home on the TV, or the TV turned off) fires `PauseStreamEvent`, and the app keeps running. A paused Steam session cost about 1 GB of RAM, ~2% of a core and bursts of up to 90% on the integrated graphics. Wolf itself, idle: ~140 MB of RAM, a few % of a core, no GPU memory.
- **Wolf's runtime folder in the image is `/run/user/wolf`**, not `/tmp/sockets` as the docs say; it's mounted from the host so app containers can reach its sockets.
- **Steam's container** (Games on Whales base image) sources `/etc/cont-init.d/*.sh` as root at start, in the shell that then starts Steam.
- **Config paths (found in the final review):** the Wolf image's start-up script (`docker/startup.sh`) sets `WOLF_CFG_FILE`, the certificate paths and the `fake-udev` path from `HOST_APPS_STATE_FOLDER` (`/etc/wolf` in the container), overriding anything set in the environment. A server's identity therefore follows the folder mounted at `/etc/wolf`.
- **Per-session data in the app home:** Wolf mounts `<app home>/udev` into the app container and deletes it when that container ends; Steam's container is named `WolfSteam_<session id>`, and creating one with a taken name removes the existing container first (`force_recreate_if_present`). Two servers sharing app homes would therefore delete each other's udev data on a switch, so the servers can't share app homes.
- **Steam's data** in the Games on Whales image lives in `~/.steam` (client, login, library); checked on spc.
- **Server IDs:** Wolf writes a `uuid` only into a config it generates. The config `setup.sh` installs has none, so spc's server reports an empty ID. Moonlight tells servers apart by this ID, so each server needs its own.

## Screens (TV)

- **Steam tile:** opens a picker in the style of Settings (same page component). Title "Steam"; two rows: **Graphics card (RTX 4060)** and **Integrated graphics card**. No details.
- Focus starts on the row used last (remembered in tv's `settings.toml`); the first row the first time.
- Up/Down move, OK starts the chosen one (the usual "Opening…" follows), Back or Home returns to the tiles.
- Failures are as for any tile: a toast if it exits within 10 s.
- **The Moonlight tile** is unchanged; its menus list two servers, `spc` and `spc-igpu`.

## Launcher architecture

| Unit | Change |
|---|---|
| `config` | A tile has either `command` or two or more `[[tile.choice]]` entries, each with `name` and `command` (same rules as a tile's command, including `--die-with-parent` for `flatpak run`). Anything else is a config error. Icons for a tile with choices come from `icon` or the first choice's Flatpak. |
| `prefs` | `[last_choice]` table: tile id → choice name. Unknown or stale names fall back to the first choice. |
| `choice_ui` (new, UI thread) | Opens the picker for a tile, moves focus, starts the chosen command through the runner as `App::Tile(index)`, saves the choice, closes on Back/Home. Like `mirroring_ui`: `open(tile)`, `is_open()`, `nav(…)`. |
| `main.rs` | Activating a tile with choices opens the picker. `on_action` routes Up/Down/Activate/Back/Home to the picker while it's open. |
| `ui/main.slint` | Shows the picker with the Settings page component; mouse hover and click work as in Settings. |

**Steam tile in `launcher.toml`:**
```toml
[[tile]]
id = "steam"
name = "Steam"
icon = "steam.svg"
color = "#1b2838"

[[tile.choice]]
name = "Graphics card (RTX 4060)"
command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc", "Steam", "--1080", "--fps", "60", "--display-mode", "fullscreen", "--multi-controller", "--no-quit-after"]

[[tile.choice]]
name = "Integrated graphics card"
command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc-igpu", "Steam", "--1080", "--fps", "60", "--display-mode", "fullscreen", "--multi-controller", "--no-quit-after"]
```

## spc architecture

| Piece | What |
|---|---|
| `wolf@.service` | Systemd template, one instance per GPU driver: `wolf@nvidia` (server **spc**) and `wolf@amdgpu` (server **spc-igpu**). Replaces `wolf.service` and the `--gpu` switch. |
| `/opt/wolf/<instance>.env` | Per server: container name (`wolf`, `wolf-igpu`), ports (RTX: as now; iGPU: each +1000, so 48984, 48989, 48999, 49010, 49100, 49200), state folder (`/etc/wolf`, `/etc/wolf-igpu`; config in its `cfg/`), runtime folder (`/run/wolf/nvidia`, `/run/wolf/amdgpu`). |
| `compose.yaml` | One file, values from the instance's env file; the instance's state folder is mounted as Wolf's `/etc/wolf`. |
| `wolf-prestart <instance>` | GPU node lookup for that instance's driver; the firewall table covers both servers' ports (Tailscale and loopback only); the NVIDIA driver volume check stays. |
| `config.toml` (template) | `setup.sh` fills in hostname and a fresh `uuid` per server. The Steam app mounts the shared Steam folder `/etc/wolf-steam` as `~/.steam` (so Steam is shared by every Moonlight device on spc, one at a time) and the Steam lock. Installed only when a server has no config, except on this migration (see below). |
| `steam-lock.sh` | Mounted read-only into each server's Steam app as `/etc/cont-init.d/05-mushtv-steam-lock.sh`. Opens `~/.steam/.mushtv-steam.lock` (in the shared folder) on a spare descriptor and waits up to 60 s for an exclusive `flock`, logging while it waits. Steam inherits the descriptor, so the lock is held until the container exits. On timeout it exits, so the container stops and the stream fails rather than two Steams sharing a folder. On a switch Wolf usually removes the other Steam container first (same name), so the lock is a backstop. |
| `wolf-sessions` | Python 3 (standard library only) service. Subscribes to both servers' event streams through their sockets, reconnecting with backoff. **Idle:** records when a session pauses, forgets it on resume or stop, and ends sessions paused for 30 minutes (`WOLF_IDLE_MINUTES`, default 30). **Shared Steam:** when a session starts or resumes on one server, ends the same session ID on the other. The decision logic is a pure function of events and time, unit-tested separately from the socket code. Logs each action to the journal. |

**Migration on spc** (one `setup.sh` run): stop and disable `wolf.service`; install the template, both instances and `wolf-sessions`; replace the RTX server's config (old one kept as `config.toml.bak`) so it gets a server ID and the lock mount; create the iGPU server's config, the state folders and the shared Steam folder. The TV then pairs with both servers again (`pair spc`, `pair spc:48989`), with the PINs approved from SSH as before, and signs in to Steam once in the shared folder (no games were installed yet).

**Cleanup in the same pass:** unpair the test client and delete its app folder (`/etc/wolf/5711466711918555371`); remove the driver build cache, the Ubuntu and helper images and the old driver build image; remove the sway pointer tweak (`~/.config/sway/custom-cfg` in the TV's Steam home).

## Testing

- **Launcher:** unit tests for choice parsing (valid; a tile with both `command` and choices; one choice; empty name), the remembered choice (round-trip; stale name), and picker navigation. Headless screenshots of the picker, including focus on the remembered row.
- **`wolf-sessions`:** unit tests over event sequences: pause then 30 minutes → stop; pause then resume → nothing; start on one server with a session on the other → stop there; unknown events ignored.
- **On spc, with the test client before it's removed:** both servers answer over Tailscale and not on the building Wi-Fi; a stream from each; a paused session ends after a shortened timeout; starting on one server ends the session on the other (its Steam container is gone) and Steam starts on the new one, with a controller working there.
- **On the TV (user):** the picker, both GPUs, Home then reopening the same GPU resumes, switching GPUs ends the other session.

## Out of scope

- Showing in the picker whether the RTX 4060 is busy.
- Picking the GPU per game.
- A GPU switch for the Moonlight tile (its menus already list both servers).
