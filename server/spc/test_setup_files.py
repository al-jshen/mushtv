"""Checks the spc setup files fit together. Run from server/spc:
python3 -m unittest -v test_setup_files (the compose test needs docker)."""

import json
import os
import shutil
import subprocess
import tempfile
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
STATE = {"nvidia": "/etc/wolf", "amdgpu": "/etc/wolf-igpu"}
SHARED_STEAM = "/etc/wolf-steam"


def env_file(instance):
    values = {}
    with open(os.path.join(HERE, f"{instance}.env")) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                key, _, value = line.partition("=")
                values[key] = value
    return values


@unittest.skipUnless(shutil.which("docker"), "needs docker compose")
class ComposeTest(unittest.TestCase):
    def render(self, instance):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, instance))
            with open(os.path.join(d, instance, "render.env"), "w") as f:
                f.write("WOLF_RENDER_NODE=/dev/dri/renderD128\n")
            with open(os.path.join(HERE, "compose.yaml")) as f:
                compose = f.read().replace("/run/wolf/${WOLF_INSTANCE}/render.env", d + "/${WOLF_INSTANCE}/render.env")
            path = os.path.join(d, "compose.yaml")
            with open(path, "w") as f:
                f.write(compose)
            out = subprocess.run(
                ["docker", "compose", "-f", path, "-p", f"wolf-{instance}",
                 "--env-file", os.path.join(HERE, f"{instance}.env"), "config", "--format", "json"],
                check=True, capture_output=True, text=True,
            ).stdout
        return json.loads(out)["services"]["wolf"]

    def test_each_server_has_its_own_state_folder(self):
        for instance, state in STATE.items():
            wolf = self.render(instance)
            mounts = {v["target"]: v["source"] for v in wolf["volumes"] if v["type"] == "bind"}
            self.assertEqual(mounts["/etc/wolf"], state, instance)
            # Wolf's startup derives config and certificate paths from its
            # state folder and overrides these, so they mustn't be relied on.
            for name in ("WOLF_CFG_FILE", "WOLF_PRIVATE_KEY_FILE", "WOLF_PRIVATE_CERT_FILE"):
                self.assertNotIn(name, wolf["environment"], instance)

    def test_servers_use_different_ports_and_names(self):
        a, b = self.render("nvidia"), self.render("amdgpu")
        self.assertNotEqual(a["container_name"], b["container_name"])
        self.assertNotEqual(a["environment"]["WOLF_HTTP_PORT"], b["environment"]["WOLF_HTTP_PORT"])


class TemplateTest(unittest.TestCase):
    def test_steam_shares_one_steam_folder_and_the_lock(self):
        with open(os.path.join(HERE, "config.toml"), "rb") as f:
            config = tomllib.load(f)
        steam = [a for p in config["profiles"] for a in p["apps"] if a["title"] == "Steam"]
        self.assertEqual(len(steam), 1)
        mounts = steam[0]["runner"]["mounts"]
        self.assertIn(f"{SHARED_STEAM}:/home/retro/.steam:rw", mounts)
        self.assertIn("/opt/wolf/steam-lock.sh:/etc/cont-init.d/05-mushtv-steam-lock.sh:ro", mounts)

    def test_the_lock_lives_in_the_shared_folder(self):
        with open(os.path.join(HERE, "steam-lock.sh")) as f:
            self.assertIn("/home/retro/.steam/.mushtv-steam.lock", f.read())

    def test_env_files_name_the_state_folders(self):
        for instance, state in STATE.items():
            self.assertEqual(env_file(instance)["WOLF_STATE_DIR"], state)


if __name__ == "__main__":
    unittest.main()
