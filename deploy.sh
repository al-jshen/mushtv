#!/usr/bin/env bash
# Install the mushtv launcher and the boot-to-kiosk setup.
#
#   sudo ./deploy.sh             install / update
#   sudo ./deploy.sh --restart   ...then restart the TV session (closes what's playing)
#   DESTDIR=/tmp/stage ./deploy.sh   dry run into a staging tree (no root needed)
#
# Build first, as yourself: (cd launcher && cargo build --release)
set -euo pipefail

repo="$(cd "$(dirname "$0")" && pwd)"
dest="${DESTDIR:-}"
restart=false

for arg in "$@"; do
  case "$arg" in
    --restart) restart=true ;;
    -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "deploy.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

die() { echo "deploy.sh: $*" >&2; exit 1; }

if [ -z "$dest" ]; then
  [ "$(id -u)" -eq 0 ] || die "run with sudo (or set DESTDIR=... for a dry run)"
  getent passwd tv >/dev/null || die "the tv user doesn't exist"
  tv_home="$(getent passwd tv | cut -d: -f6)"
else
  $restart && die "--restart can't be combined with DESTDIR"
  tv_home=/home/tv
fi

bin="$repo/launcher/target/release/mushtv-launcher"
config="$repo/launcher/launcher.toml"
[ -x "$bin" ] || die "no build found; run: (cd launcher && cargo build --release)"
"$bin" --check-config "$config" || die "fix $config first"

# Create a directory if it's missing; never change an existing one's mode.
ensure_dir() {
  [ -d "$1" ] || install -d -m 755 "$1"
}

# Replace a root-owned file, showing what changed and keeping a .bak of a
# modified copy.
install_file() {  # mode src dst
  local mode="$1" src="$2" dst="$3"
  ensure_dir "$(dirname "$dst")"
  if [ -f "$dst" ] && ! cmp -s "$src" "$dst"; then
    echo "== updating $dst:"
    diff -u "$dst" "$src" || true
    cp -p "$dst" "$dst.bak"
  fi
  install -m "$mode" "$src" "$dst"
}

# Run a command as tv (as yourself in a DESTDIR dry run).
as_tv() {
  if [ -z "$dest" ]; then runuser -u tv -- "$@"; else "$@"; fi
}

# Replace a file in tv's home. Everything happens as tv, with the content
# piped in, so a symlink tv planted can't make root read or write anything
# tv couldn't. Renames replace links instead of writing through them.
install_tv_file() {  # src dst
  as_tv sh -c '
    dst="$1"
    cat > "$dst.new"
    chmod 644 "$dst.new"
    if [ -e "$dst" ] && ! cmp -s "$dst.new" "$dst"; then
      echo "== updating $dst:"
      diff -u "$dst" "$dst.new" || true
      cp -- "$dst" "$dst.bak.new" && mv -f -- "$dst.bak.new" "$dst.bak"
    fi
    mv -f -- "$dst.new" "$dst"
  ' sh "$2" < "$1"
}

share="$dest/usr/local/share/mushtv"
install -D -m 755 "$bin" "$dest/usr/local/bin/mushtv-launcher"
install_file 644 "$repo/launcher/assets/fonts/LICENSE-Inter.txt" "$share/LICENSE-Inter.txt"
# Tile icons and an optional background.png/jpg go in launcher/assets/share/.
if [ -d "$repo/launcher/assets/share" ]; then
  for f in "$repo"/launcher/assets/share/*; do
    [ -f "$f" ] && install_file 644 "$f" "$share/$(basename "$f")"
  done
fi
install_file 644 "$config" "$dest/etc/mushtv/launcher.toml"
install_file 644 "$repo/deploy/autologin.conf" "$dest/etc/systemd/system/getty@tty1.service.d/autologin.conf"
if [ -n "$dest" ]; then
  ensure_dir "$dest$tv_home"
fi
[ -d "$dest$tv_home" ] || die "tv's home $tv_home doesn't exist"
install_tv_file "$repo/deploy/bash_profile" "$dest$tv_home/.bash_profile"

if [ -z "$dest" ]; then
  systemctl daemon-reload
  if $restart; then
    echo "restarting the TV session"
    systemctl restart getty@tty1
  else
    echo "installed. The running launcher keeps the old version until the TV session restarts:"
    echo "  sudo systemctl restart getty@tty1   (or: sudo ./deploy.sh --restart)"
  fi
else
  echo "staged into $dest"
fi
