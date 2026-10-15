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
