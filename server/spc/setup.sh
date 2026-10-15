#!/bin/bash
# Sets up the two Wolf game streaming servers on spc (see README.md). Run from
# your own account on spc, in a copy of this directory:
#   sudo bash setup.sh
# Safe to run again: it reinstalls the files and restarts Wolf, but keeps each
# server's config (with its pairings) once it has one.
set -euo pipefail
cd "$(dirname "$0")"

if [ "$#" -ne 0 ]; then
	echo "usage: sudo bash setup.sh" >&2
	exit 1
fi
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
# One server before there were two: wolf.service, with a GPU drop-in.
if [ -e /etc/systemd/system/wolf.service ]; then
	systemctl disable --now wolf.service
	rm -rf /etc/systemd/system/wolf.service /etc/systemd/system/wolf.service.d
fi
install -d -m 755 /opt/wolf
install -m 644 compose.yaml nvidia-driver.Dockerfile nvidia.env amdgpu.env \
	steam-lock.sh wolf_sessions.py /opt/wolf/
install -m 755 wolf-prestart /opt/wolf/
install -m 644 wolf@.service wolf-sessions.service /etc/systemd/system/
# Each server's state (config, pairings, certificate, each device's app
# homes) is readable only by root. The shared Steam folder (mounted as
# ~/.steam by both servers' Steam, which runs as you) holds your Steam login.
install -d -m 700 /etc/wolf /etc/wolf-igpu
install -d -m 700 -o "$user" -g "$user" /etc/wolf-steam

# Each server's config: from the template, with its name and a fresh server
# ID. A config without the Steam lock predates two servers: keep it as
# config.toml.bak and replace it (devices then pair again).
config_for() { # <dir> <hostname>
	local cfg="$1/config.toml"
	install -d -m 700 "$1"
	if [ -e "$cfg" ] && grep -q steam-lock "$cfg"; then
		return
	fi
	if [ -e "$cfg" ]; then
		cp -p "$cfg" "$cfg.bak"
		echo "$cfg: replaced (old one kept as config.toml.bak); pair devices again"
	fi
	sed -e "s/@HOSTNAME@/$2/" -e "s/@UUID@/$(cat /proc/sys/kernel/random/uuid)/" config.toml >"$cfg"
	chmod 600 "$cfg"
}
config_for /etc/wolf/cfg spc
config_for /etc/wolf-igpu/cfg spc-igpu

systemctl daemon-reload
systemctl enable wolf@nvidia.service wolf@amdgpu.service wolf-sessions.service
echo "Starting both servers. After a driver update the first start rebuilds the"
echo "NVIDIA driver volume, which takes a few minutes."
systemctl restart wolf@nvidia.service wolf@amdgpu.service wolf-sessions.service
systemctl --no-pager --lines=3 status wolf@nvidia.service wolf@amdgpu.service wolf-sessions.service
