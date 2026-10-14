# Game streaming server (spc)

spc is the gaming PC (RTX 4060) that the TV streams games from with Moonlight. Sometimes someone else is using it at its own monitor, so it runs [Wolf](https://github.com/games-on-whales/wolf) rather than Sunshine. Sunshine streams the one physical screen. Wolf gives each Moonlight client its own invisible desktop in Docker containers on the same GPU, so you can play while the desktop is in use:

- **Display:** nothing appears on spc's monitor.
- **Controllers:** Wolf's virtual controllers sit on a seat of their own (`85-wolf.rules`), so they can't drive the desktop.
- **Audio:** stays inside Wolf's containers.

Steam runs in its own container with its own home. Sign in once on the TV with one of the accounts in the Steam family, and both libraries appear. Games download separately from any Steam on spc's desktop.

**Sharing the GPU:** streams and the desk share the RTX 4060 and its 8 GB of video memory. A game at the desk can take most of that memory (one took 5.8 GB), and then a stream can't even start: Wolf's encoder fails with `CUDA_ERROR_OUT_OF_MEMORY` in `docker logs wolf`, and Moonlight says "No video received from host". Streaming alongside desktop use (browsing, video) is fine; alongside a game, only if both are light. Wolf uses one GPU for everything, so it can't fall back to the Ryzen's iGPU on its own; see **Choosing the GPU** below.

## What's here

| File | Installed to | What it does |
|---|---|---|
| `setup.sh` | — | One-time setup, run with sudo on spc. Re-runnable. |
| `compose.yaml` | `/opt/wolf/` | The Wolf container. |
| `wolf.service` | `/etc/systemd/system/` | Starts Wolf at boot, after `wolf-prestart`. |
| `wolf-prestart` | `/opt/wolf/` | Firewall, GPU node, driver volume; see below. |
| `nvidia-driver.Dockerfile` | `/opt/wolf/` | Builds the NVIDIA driver for Wolf's containers (from Games on Whales). |
| `85-wolf.rules` | `/etc/udev/rules.d/` | Wolf's virtual input rules (from Wolf). |
| `config.toml` | `/etc/wolf/cfg/` | Wolf's config, only when none exists: Wolf adds paired clients to it. |

`setup.sh` installs Docker and adds you to the `docker` group, which is root-equivalent. Wolf keeps its state in `/etc/wolf`: config, pairing certificates, and each app's home folder (Steam's is under `profile_data/`). Only root can read it.

Before every start, `wolf-prestart`:

- **Firewall:** opens Wolf's ports (TCP 47984, 47989, 48010; UDP 47999, 48100, 48200) on Tailscale and loopback only. Its own nftables table `inet wolf` drops them everywhere else, because spc's Wi-Fi is shared with the building. ufw stays off as before.
- **GPU node:** finds the chosen GPU's render node (their numbering follows driver probe order) and passes it to Wolf.
- **Driver volume:** rebuilds it when the loaded NVIDIA driver no longer matches. Wolf's containers can't use the host's driver directly. Ubuntu updates the driver on its own, so without this Wolf would show a black screen after the next reboot.

## Setup

From this box (substrate), copy the directory over and run it with sudo on spc:

```
scp -r server/spc jshen@spc:wolf-setup
ssh -t jshen@spc sudo bash wolf-setup/setup.sh
```

The first start pulls Wolf and builds the driver volume, which takes a few minutes. The first Steam launch also pulls the Steam image (a few GB), and the stream stays black until it's done.

To pair a Moonlight client, start pairing on the client and note its PIN. Then look for the PIN link in Wolf's log (`docker logs wolf | grep -i pin`) and submit the PIN from spc at `http://localhost:47989/pin/#<secret>`.

## Using it

- **Address:** clients reach it over Tailscale as `spc`. Moonlight treats Tailscale addresses as remote and keeps packets small enough for it.
- **Leaving:** closing Moonlight (or pressing Home on the TV) disconnects but leaves the game running; reconnecting resumes it. To end the session, exit Steam (Power → Exit Steam) or quit the app from Moonlight.
- **Logs:** `journalctl -u wolf` on spc, or `docker logs -f wolf`.
- **Choosing the GPU:** `sudo bash setup.sh --gpu amdgpu` runs Wolf on the Ryzen's built-in AMD graphics. It uses system RAM, so streams never touch the RTX 4060 or a game at the desk, but it's weak: fine for light games, not modern 3D. `--gpu nvidia` goes back to the RTX 4060 (the default). The choice is a drop-in, `/etc/systemd/system/wolf.service.d/gpu.conf`, and Wolf's log names the encoder it picked (`nvcodec` or `va`).
- **Stop or start:** `sudo systemctl stop wolf` and `sudo systemctl start wolf`. Remove it with `sudo systemctl disable --now wolf`, then delete the installed files above.
