"""Checks the spc setup files fit together. Run from server/spc:
python3 -m unittest -v test_setup_files (the compose test needs docker)."""

import json
import os
import re
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

    def test_steam_gets_the_time_zone_and_no_proton_log(self):
        # The Steam image's own time zone is Europe/London; setup.sh fills in
        # spc's. Proton's debug log grew to 4 GB in one evening.
        with open(os.path.join(HERE, "config.toml"), "rb") as f:
            config = tomllib.load(f)
        steam = [a for p in config["profiles"] for a in p["apps"] if a["title"] == "Steam"]
        env = steam[0]["runner"]["env"]
        self.assertIn("TZ=@TZ@", env)
        self.assertFalse([e for e in env if e.startswith("PROTON_LOG")], env)

    def test_the_lock_lives_in_the_shared_folder(self):
        with open(os.path.join(HERE, "steam-lock.sh")) as f:
            self.assertIn("/home/retro/.steam/.mushtv-steam.lock", f.read())

    def test_env_files_name_the_state_folders(self):
        for instance, state in STATE.items():
            self.assertEqual(env_file(instance)["WOLF_STATE_DIR"], state)


class ConfigForTest(unittest.TestCase):
    """setup.sh's config_for, run on its own in a temporary folder."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        with open(os.path.join(HERE, "setup.sh")) as f:
            self.function = re.search(r"^config_for\(\) \{.*?^\}$", f.read(), re.M | re.S).group(0)
        self.cfg = os.path.join(self.tmp.name, "cfg", "config.toml")

    def config_for(self, tz):
        subprocess.run(
            ["bash", "-c", f'set -euo pipefail; tz="{tz}"\n{self.function}\nconfig_for "{os.path.dirname(self.cfg)}" spc'],
            cwd=HERE, check=True, capture_output=True, text=True,
        )
        with open(self.cfg) as f:
            return f.read()

    def steam_env(self, text):
        config = tomllib.loads(text)
        return [a for p in config["profiles"] for a in p["apps"] if a["title"] == "Steam"][0]["runner"]["env"]

    def test_a_new_config_gets_the_time_zone(self):
        text = self.config_for("America/New_York")
        self.assertNotRegex(text, r"@[A-Z]+@")
        self.assertIn("TZ=America/New_York", self.steam_env(text))

    def test_a_kept_config_gets_the_time_zone_instead_of_the_proton_log(self):
        # As Wolf rewrites it: single quotes, one line.
        os.makedirs(os.path.dirname(self.cfg))
        with open(self.cfg, "w") as f:
            f.write("""hostname = 'spc'
[[profiles]]
id = 'moonlight-profile-id'
    [[profiles.apps]]
    title = 'Steam'
        [profiles.apps.runner]
        env = [ 'PROTON_LOG=1', 'RUN_SWAY=true', 'GOW_REQUIRED_DEVICES=/dev/input/* /dev/dri/* /dev/nvidia*' ]
        mounts = [ '/opt/wolf/steam-lock.sh:/etc/cont-init.d/05-mushtv-steam-lock.sh:ro' ]
""")
        env = self.steam_env(self.config_for("America/New_York"))
        self.assertEqual(env[:2], ["TZ=America/New_York", "RUN_SWAY=true"])
        env = self.steam_env(self.config_for("Europe/Paris"))
        self.assertEqual(env[:2], ["TZ=Europe/Paris", "RUN_SWAY=true"])


class SharedHomeTest(unittest.TestCase):
    """steam-lock.sh also shares ~/.config and ~/.local (where native Linux
    games save) between the servers, through the shared ~/.steam, and hides
    an idle pointer."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = os.path.join(self.tmp.name, "home")
        os.makedirs(os.path.join(self.home, ".steam"))  # the shared mount
        self.sway = os.path.join(self.tmp.name, "sway-config.d")
        with open(os.path.join(HERE, "steam-lock.sh")) as f:
            script = f.read().replace("/home/retro", self.home).replace("/etc/sway/config.d", self.sway)
        self.script = os.path.join(self.tmp.name, "init.sh")
        with open(self.script, "w") as f:
            f.write(script)

    def run_init(self):
        """Sources the script like the image's entrypoint does (set -e)."""
        env = dict(os.environ, PUID=str(os.getuid()), PGID=str(os.getgid()))
        r = subprocess.run(
            ["bash", "-c", f'set -e; gow_log() {{ echo "$*"; }}; source "{self.script}"; echo sourced-ok'],
            env=env, capture_output=True, text=True,
        )
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("sourced-ok", r.stdout)

    def write(self, rel, text):
        path = os.path.join(self.home, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)

    def read(self, rel):
        with open(os.path.join(self.home, rel)) as f:
            return f.read()

    def assertShared(self, d):
        link = os.path.join(self.home, "." + d)
        self.assertTrue(os.path.islink(link), link)
        self.assertEqual(os.readlink(link), f".steam/mushtv-home/{d}")

    def test_first_server_seeds_the_shared_folders(self):
        self.write(".config/game/save", "progress")
        self.write(".local/share/game/save", "more")
        self.run_init()
        self.assertShared("config")
        self.assertShared("local")
        self.assertEqual(self.read(".config/game/save"), "progress")
        self.assertEqual(self.read(".steam/mushtv-home/local/share/game/save"), "more")
        self.assertEqual(self.read(".config.mushtv-old/game/save"), "progress")

    def test_second_server_uses_the_shared_copy_and_keeps_its_own(self):
        self.write(".steam/mushtv-home/config/game/save", "rtx")
        self.write(".config/game/save", "igpu")
        self.run_init()
        self.assertShared("config")
        self.assertEqual(self.read(".config/game/save"), "rtx")
        self.assertEqual(self.read(".config.mushtv-old/game/save"), "igpu")

    def test_running_again_changes_nothing(self):
        self.write(".config/game/save", "progress")
        self.run_init()
        self.run_init()
        self.assertShared("config")
        self.assertEqual(sorted(n for n in os.listdir(self.home) if n.startswith(".config")),
                         [".config", ".config.mushtv-old"])

    def test_a_new_home_gets_empty_shared_folders(self):
        self.run_init()
        self.assertShared("config")
        self.assertShared("local")
        self.assertTrue(os.path.isdir(os.path.join(self.home, ".config")))

    def test_sway_hides_an_idle_pointer(self):
        self.run_init()
        settings = [open(os.path.join(self.sway, n)).read() for n in os.listdir(self.sway)]
        self.assertEqual(settings, ["seat * hide_cursor 1000\n"])

    def test_an_old_backup_is_not_overwritten(self):
        self.write(".config.mushtv-old/keep", "first backup")
        self.write(".steam/mushtv-home/config/x", "shared")
        self.write(".config/game/save", "local")
        self.run_init()
        self.assertEqual(self.read(".config.mushtv-old/keep"), "first backup")
        backups = [n for n in os.listdir(self.home) if n.startswith(".config.mushtv-old.")]
        self.assertEqual(len(backups), 1)
        self.assertEqual(self.read(f"{backups[0]}/game/save"), "local")


if __name__ == "__main__":
    unittest.main()
