#!/bin/bash
# Sets up the Wolf game streaming server on spc (see README.md). Run from
# your own account on spc, in a copy of this directory:
#   sudo bash setup.sh [--gpu nvidia|amdgpu]
# --gpu picks the GPU Wolf runs on: nvidia (the RTX 4060, the default) or
# amdgpu (the Ryzen's iGPU). Without it, the current choice stays.
# Safe to run again: it reinstalls the files and restarts Wolf, but keeps
# Wolf's config (with its pairings) once there is one.
set -euo pipefail
cd "$(dirname "$0")"

gpu=
case "${1:-}" in
"") ;;
--gpu)
	gpu=${2:-}
	if [ "$gpu" != nvidia ] && [ "$gpu" != amdgpu ]; then
		echo "setup.sh: --gpu takes nvidia or amdgpu" >&2
		exit 1
	fi
	;;
*)
	echo "usage: sudo bash setup.sh [--gpu nvidia|amdgpu]" >&2
	exit 1
	;;
esac

if [ "$(id -u)" -ne 0 ]; then
	echo "setup.sh: run with sudo" >&2
	exit 1
fi
user=${SUDO_USER:?setup.sh: run with sudo from your own account}

echo "== Docker"
apt-get update -q
apt-get install -y -q --no-install-recommends docker.io docker-compose-v2 docker-buildx
# Lets $user manage Wolf without sudo (the docker group is root-equivalent).
usermod -aG docker "$user"

echo "== Virtual input rules"
install -m 644 85-wolf.rules /etc/udev/rules.d/85-wolf.rules
udevadm control --reload-rules
udevadm trigger /dev/uinput /dev/uhid

echo "== Wolf"
install -d -m 755 /opt/wolf
install -m 644 compose.yaml nvidia-driver.Dockerfile /opt/wolf/
install -m 755 wolf-prestart /opt/wolf/
install -m 644 wolf.service /etc/systemd/system/wolf.service
# The GPU choice lives in a drop-in, so reinstalling wolf.service keeps it.
dropin=/etc/systemd/system/wolf.service.d/gpu.conf
if [ "$gpu" = amdgpu ]; then
	install -d -m 755 "$(dirname "$dropin")"
	printf '[Service]\nEnvironment=WOLF_GPU=amdgpu\n' >"$dropin"
elif [ "$gpu" = nvidia ]; then
	rm -f "$dropin"
fi
# Wolf's state includes each app's home folder, with your Steam login in
# it, so only root may look inside.
install -d -m 700 /etc/wolf /etc/wolf/cfg
if [ ! -e /etc/wolf/cfg/config.toml ]; then
	install -m 600 config.toml /etc/wolf/cfg/config.toml
fi
systemctl daemon-reload
systemctl enable wolf.service
echo "Starting Wolf. The first start builds the NVIDIA driver volume and"
echo "downloads Wolf, which takes a few minutes."
systemctl restart wolf.service
systemctl --no-pager --lines=5 status wolf.service
