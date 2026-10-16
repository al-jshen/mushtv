# Game streaming servers (spc)

spc is the gaming PC (RTX 4060) that the TV streams games from with Moonlight. Sometimes someone else is using it at its own monitor, so it runs [Wolf](https://github.com/games-on-whales/wolf) rather than Sunshine. Sunshine streams the one physical screen. Wolf gives each Moonlight client its own invisible desktop in Docker containers, so you can play while the desktop is in use:

- **Display:** nothing appears on spc's monitor.
- **Controllers:** Wolf's virtual controllers sit on a seat of their own (`85-wolf.rules`), so they can't drive the desktop.
- **Audio:** stays inside Wolf's containers.

## Two servers, one Steam

| Server (Moonlight name) | GPU | Ports (TCP; UDP) | systemd unit, container |
|---|---|---|---|
| **spc** | RTX 4060 | 47984, 47989, 48010; 47999, 48100, 48200 | `wolf@nvidia`, `wolf`, state in `/etc/wolf` |
| **spc-igpu** | Ryzen's integrated graphics | 48984, 48989, 49010; 48999, 49100, 49200 | `wolf@amdgpu`, `wolf-igpu`, state in `/etc/wolf-igpu` |

- **Why two:** the RTX 4060 and its 8 GB of video memory are shared with the desk. A game there can take most of it (one took 5.8 GB), and then a stream can't even start: Wolf's encoder fails with `CUDA_ERROR_OUT_OF_MEMORY` (`docker logs wolf`) and Moonlight says "No video received from host". The integrated graphics uses system RAM, so **spc-igpu** never touches the RTX 4060 or a game at the desk; it's weak, though: fine for light games (Overcooked 2 and the like), not modern 3D. Wolf can't mix GPUs in one server, hence two.
- **Each server has its own state folder.** Wolf derives its config, certificate and other paths from it, and keeps each device's app homes there (named after the device's Moonlight certificate). Separate folders keep the two servers' identities and per-session data apart.
- **One Steam:** both servers' Steam mount one folder, `/etc/wolf-steam`, as `~/.steam`, where Steam keeps its client, your login and the games. So there is one login and games install once, whichever GPU you pick. It's shared by every Moonlight device on spc (today, the TV), one at a time; a Steam account can only run one game at a time anyway. Games download separately from any Steam on spc's desktop.
- **One at a time:** Wolf names a device's Steam container the same on both servers (`WolfSteam_<session>`), so starting Steam on one server removes the other server's Steam container first; that game is ended without saving. `steam-lock.sh`, mounted into Steam's container on both servers, is a backstop: it holds a lock (`.mushtv-steam.lock` in the shared folder) while Steam runs, and a new Steam waits up to 60 s for any other to close, then gives up rather than start alongside it.
- **Session guard (`wolf_sessions.py`, service `wolf-sessions`):** watches both servers' event streams. When a device starts or resumes on one server, it ends that device's session on the other, so nothing is left behind there. A session paused for 30 minutes (Home on the TV, or the TV switched off) is ended too; the next start is a fresh Steam. Change the time with `Environment=WOLF_IDLE_MINUTES=…` in a drop-in for `wolf-sessions`. Restarting the guard forgets sessions that were already paused.

## What's here

| File | Installed to | What it does |
|---|---|---|
| `setup.sh` | — | One-time setup, run with sudo on spc. Re-runnable. |
| `compose.yaml` | `/opt/wolf/` | The Wolf container; values from the server's env file. |
| `nvidia.env`, `amdgpu.env` | `/opt/wolf/` | Per server: container name, config folder, ports. |
| `wolf@.service` | `/etc/systemd/system/` | One instance per server; runs `wolf-prestart`, then Wolf. |
| `wolf-prestart` | `/opt/wolf/` | Firewall, GPU node, driver volume; see below. |
| `nvidia-driver.Dockerfile` | `/opt/wolf/` | Builds the NVIDIA driver for Wolf's containers (from Games on Whales). |
| `steam-lock.sh` | `/opt/wolf/` | The Steam lock, mounted into Steam's container. |
| `wolf_sessions.py`, `wolf-sessions.service` | `/opt/wolf/`, `/etc/systemd/system/` | The session guard. Tests: `python3 -m unittest -v test_wolf_sessions` here. |
| `85-wolf.rules` | `/etc/udev/rules.d/` | Wolf's virtual input rules (from Wolf). |
| `config.toml` | `/etc/wolf/cfg/`, `/etc/wolf-igpu/cfg/` | Template for each server's config (name, fresh server ID, shared Steam and lock mounts), installed only where none exists: Wolf adds paired clients to it. |
| `test_setup_files.py` | — | Checks these files fit together (`python3 -m unittest -v test_setup_files`; the compose part needs docker). |

`setup.sh` installs Docker and adds you to the `docker` group, which is root-equivalent. The servers' state in `/etc/wolf` and `/etc/wolf-igpu` (configs, pairing certificates, each device's app homes) is readable only by root; `/etc/wolf-steam` (your Steam login and games) only by you and root.

Before each server starts, `wolf-prestart <gpu>`:

- **Firewall:** both servers' ports answer only on Tailscale and loopback. The nftables table `inet wolf` drops them everywhere else, because spc's Wi-Fi is shared with the building. ufw stays off as before.
- **GPU node:** finds that server's render node (their numbering follows driver probe order) and passes it to Wolf.
- **Driver volume:** rebuilds it when the loaded NVIDIA driver no longer matches (both servers use it; a lock keeps them from rebuilding at once). Wolf's containers can't use the host's driver directly, and Ubuntu updates the driver on its own, so without this Wolf would show a black screen after the next reboot.

## Setup

From this box (substrate), copy the directory over and run it with sudo on spc:

```
rsync -a --delete server/spc/ jshen@spc:wolf-setup/
ssh -t jshen@spc sudo bash wolf-setup/setup.sh
```

The first start pulls Wolf and builds the driver volume, which takes a few minutes. The first Steam launch on a server pulls the Steam image (a few GB), and the stream stays black until it's done.

**Upgrading from one server** (`wolf.service`): `setup.sh` removes it and replaces the old config (kept as `config.toml.bak`) so the server gets an ID and the Steam mounts. Devices then pair with both servers again, and Steam asks you to sign in once in the new shared folder (the old per-device copies under `/etc/wolf/*/Steam/.steam` are no longer used and can be deleted).

**Pairing** a Moonlight client: pair with `spc` and with `spc:48989` (the second server's port). Each shows a PIN; find the PIN link in that server's log (`docker logs wolf | grep -i pin`, `docker logs wolf-igpu | grep -i pin`) and submit the PIN from spc at `http://localhost:47989/pin/#<secret>` (or `:48989`).

**Pairing the TV** (it has no Moonlight menus): run these on the TV box (substrate) while a watcher on spc approves the PIN, or submit it by hand as above. If Moonlight still remembers an old pairing with a server whose pairings were reset, it keeps using that and never asks for a PIN; clear its saved servers first (only the `[hosts]` section; a `.bak` is kept):
```
sudo pkill -u tv -x moonlight
sudo -u tv sed -i.bak '/^\[hosts\]$/,/^\[/{/^\[hosts\]$/d;/^\[/!d}' "/home/tv/.var/app/com.moonlight_stream.Moonlight/config/Moonlight Game Streaming Project/Moonlight.conf"
sudo -u tv env XDG_RUNTIME_DIR=/run/user/1002 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1002/bus QT_QPA_PLATFORM=offscreen timeout 120 flatpak run --die-with-parent com.moonlight_stream.Moonlight pair spc --pin 1234
sudo -u tv env XDG_RUNTIME_DIR=/run/user/1002 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1002/bus QT_QPA_PLATFORM=offscreen timeout 120 flatpak run --die-with-parent com.moonlight_stream.Moonlight pair spc:48989 --pin 1234
```
(Headless pairing doesn't exit by itself; `timeout` ends it.)

## Using it

- **Address:** clients reach both over Tailscale (`spc`, `spc:48989`). Moonlight treats Tailscale addresses as remote and keeps packets small enough for it.
- **Leaving:** closing Moonlight (or Home on the TV) disconnects but leaves the game running; reconnecting to the same server resumes it, within 30 minutes. To end it, exit Steam (Power → Exit Steam) or quit the app from Moonlight.
- **Moonlight shows an old "spc" next to the new one** (after upgrading from one server): delete the old entry in Moonlight's menus.
- **Controllers are presented to games as Xbox controllers.** Steam Input (Steam's remapping, on by default for PlayStation controllers) needs to create virtual devices, which doesn't work in Wolf's containers ([Wolf issue #81](https://github.com/games-on-whales/wolf/issues/81)): Steam's menus see the controller but games don't. Wolf's per-device `controllers_override` avoids it. It's set for the TV on both servers through Wolf's API, so it's stored in each server's config (and lost if that config is replaced):
  ```
  docker run --rm --user 0 -v /run/wolf:/run/wolf curlimages/curl:8.11.1 -s --unix-socket /run/wolf/nvidia/sockets/wolf.sock \
    -H 'Content-Type: application/json' http://localhost/api/v1/clients/settings \
    -d '{"client_id":"5334278758390175681","settings":{"controllers_override":["XBOX","XBOX","XBOX","XBOX"]}}'
  ```
  (and the same with `amdgpu`; `GET /api/v1/clients` lists device IDs).
- **Logs:** `docker logs -f wolf`, `docker logs -f wolf-igpu`; `sudo journalctl -u wolf@nvidia -u wolf@amdgpu -u wolf-sessions`.
- **Stop or start:** `sudo systemctl stop wolf@amdgpu` (or `wolf@nvidia`), and `start`. Remove everything with `sudo systemctl disable --now wolf@nvidia wolf@amdgpu wolf-sessions`, then delete the installed files above.
