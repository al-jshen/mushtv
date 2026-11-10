"""Turn on jellyfin-mpv-shim's Game Controller setting (input_gamepad).

It's off by default everywhere but SteamOS, and while it's off the app never
opens the controller, so the controller does nothing in Jellyfin.

deploy.sh runs this as tv, with this file on stdin:

    python3 -I - CONF_JSON [--check]

--check only reads. Exit status: 0 already on; 10 off (now on, without
--check); 3 no settings file yet; 4 the file isn't readable JSON (left alone).
"""
import json
import os
import sys

path = sys.argv[1]
check_only = "--check" in sys.argv[2:]

try:
    with open(path, encoding="utf-8") as f:
        conf = json.load(f)
except FileNotFoundError:
    sys.exit(3)
except (OSError, ValueError) as e:
    print(f"{path}: {e}", file=sys.stderr)
    sys.exit(4)
if not isinstance(conf, dict):
    print(f"{path}: not a JSON object", file=sys.stderr)
    sys.exit(4)

if conf.get("input_gamepad") is True:
    sys.exit(0)

if not check_only:
    conf["input_gamepad"] = True
    tmp = path + ".mushtv-new"
    with open(tmp, "w", encoding="utf-8") as f:
        # Written the way the app writes it, so the file looks the same.
        json.dump(conf, f, indent=4, sort_keys=True)
    os.chmod(tmp, os.stat(path).st_mode & 0o7777)
    os.replace(tmp, path)
sys.exit(10)
