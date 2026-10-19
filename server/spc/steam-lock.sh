# mushtv: sourced as root by the Steam container's entrypoint before Steam
# starts; both servers' config mounts it as
# /etc/cont-init.d/05-mushtv-steam-lock.sh. Both Wolf servers mount one shared
# Steam folder as ~/.steam, so wait for any other Steam using it to exit. When
# switching servers Wolf usually removes the other server's Steam container
# itself (same name), so this is a backstop. The lock stays held on
# descriptor 9, which Steam inherits, until this container exits.
exec 9>>/home/retro/.steam/.mushtv-steam.lock
if ! flock -n 9; then
	gow_log "[mushtv] waiting up to 60 s for the other server's Steam to close"
	if ! flock -w 60 9; then
		gow_log "[mushtv] the other server's Steam is still running; not starting"
		exit 1
	fi
fi

# Native Linux games save in ~/.config and ~/.local, which are in each server's
# own app home, so a game would continue on one GPU only. Keep them in the
# shared Steam folder instead, linked from here. The first server to get here
# seeds them with its own; a home's earlier copy is kept as .<dir>.mushtv-old.
for mushtv_dir in config local; do
	mushtv_link=/home/retro/.$mushtv_dir
	mushtv_shared=/home/retro/.steam/mushtv-home/$mushtv_dir
	if [ -L "$mushtv_link" ]; then
		continue
	fi
	if [ ! -d "$mushtv_shared" ]; then
		# Copied aside and renamed, so a failed copy is never taken for a seed.
		rm -rf "$mushtv_shared.new"
		mkdir -p "$mushtv_shared.new"
		if [ -d "$mushtv_link" ]; then
			cp -a "$mushtv_link/." "$mushtv_shared.new/"
		fi
		chown "${PUID:-1000}:${PGID:-1000}" "${mushtv_shared%/*}" "$mushtv_shared.new"
		mv "$mushtv_shared.new" "$mushtv_shared"
	fi
	if [ -e "$mushtv_link" ]; then
		mushtv_old=$mushtv_link.mushtv-old
		if [ -e "$mushtv_old" ]; then
			mushtv_old=$mushtv_old.$(date +%s)
		fi
		mv "$mushtv_link" "$mushtv_old"
	fi
	ln -s ".steam/mushtv-home/$mushtv_dir" "$mushtv_link"
	chown -h "${PUID:-1000}:${PGID:-1000}" "$mushtv_link"
	gow_log "[mushtv] ~/.$mushtv_dir is now the shared one"
done
unset mushtv_dir mushtv_link mushtv_shared mushtv_old
