//! Launcher configuration: loading, validation, and tile icon resolution.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use evdev::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockStyle {
    H24,
    H12,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub clock: ClockStyle,
    pub home_keys: Vec<KeyCode>,
    pub guide_hold: Duration,
    pub tiles: Vec<Tile>,
    /// The name Macs see in Screen Mirroring.
    pub mirroring_name: String,
}

#[derive(Debug, Clone)]
pub struct Tile {
    pub id: String,
    pub name: String,
    /// Empty when the tile has choices.
    pub command: Vec<String>,
    /// Ways to start the tile, offered when it's opened (two or more), or
    /// empty for a tile that just runs `command`.
    pub choices: Vec<Choice>,
    pub stop_command: Option<Vec<String>>,
    pub icon: Option<PathBuf>,
    pub color: Option<(u8, u8, u8)>,
}

/// One way to start a tile that offers a choice (e.g. which GPU).
#[derive(Debug, Clone)]
pub struct Choice {
    pub name: String,
    pub command: Vec<String>,
}

/// Where tile icons are looked up.
#[derive(Debug, Clone)]
pub struct IconRoots {
    /// Relative `icon = "..."` paths resolve against this directory.
    pub assets: PathBuf,
    /// hicolor theme roots of Flatpak installations, e.g.
    /// `/var/lib/flatpak/exports/share/icons/hicolor`.
    pub flatpak_icons: Vec<PathBuf>,
}

pub fn load(path: &Path, icon_roots: &IconRoots) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read {}", path.display()))?;
    parse(&text, icon_roots).with_context(|| format!("invalid config {}", path.display()))
}

pub fn parse(text: &str, icon_roots: &IconRoots) -> Result<Config> {
    let raw: RawConfig = toml::from_str(text)?;

    let clock = match raw.launcher.clock.as_deref() {
        None | Some("24h") => ClockStyle::H24,
        Some("12h") => ClockStyle::H12,
        Some(other) => bail!("[launcher] clock must be \"24h\" or \"12h\", not {other:?}"),
    };

    let home_keys = match raw.launcher.home_keys {
        None => vec![KeyCode::KEY_HOMEPAGE],
        Some(names) => {
            if names.is_empty() {
                bail!("[launcher] home_keys must list at least one key");
            }
            names
                .iter()
                .map(|n| {
                    n.parse::<KeyCode>().map_err(|_| {
                        anyhow::anyhow!("[launcher] home_keys: unknown key name {n:?}")
                    })
                })
                .collect::<Result<_>>()?
        }
    };

    let guide_hold_ms = raw.launcher.guide_hold_ms.unwrap_or(1000);
    if !(300..=5000).contains(&guide_hold_ms) {
        bail!("[launcher] guide_hold_ms must be between 300 and 5000, not {guide_hold_ms}");
    }

    let mirroring_name = match raw.mirroring.name {
        None => "mushtv".to_string(),
        Some(name) if name.trim().is_empty() => bail!("[mirroring] name must not be empty"),
        Some(name) => name.trim().to_string(),
    };

    if raw.tiles.is_empty() {
        bail!("the config needs at least one [[tile]]");
    }
    let mut tiles: Vec<Tile> = Vec::with_capacity(raw.tiles.len());
    for (i, t) in raw.tiles.into_iter().enumerate() {
        let what = format!("tile {} ({:?})", i + 1, t.name);
        if t.id.is_empty()
            || !t
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            bail!("{what}: id {:?} may only contain a-z, 0-9 and '-'", t.id);
        }
        if tiles.iter().any(|other| other.id == t.id) {
            bail!("{what}: duplicate id {:?}", t.id);
        }
        let (command, choices) = match (t.command, t.choices.len()) {
            (Some(_), n) if n > 0 => {
                bail!("{what}: use either command or [[tile.choice]], not both")
            }
            (Some(command), _) => {
                if command.is_empty() {
                    bail!("{what}: command must not be empty");
                }
                (command, Vec::new())
            }
            (None, 0) => bail!("{what}: needs a command or two or more [[tile.choice]]"),
            (None, 1) => {
                bail!("{what}: needs two or more [[tile.choice]] (with one, use command)")
            }
            (None, _) => {
                let mut choices: Vec<Choice> = Vec::new();
                for (n, c) in t.choices.into_iter().enumerate() {
                    let name = c.name.trim().to_string();
                    if name.is_empty() {
                        bail!("{what}: choice {} needs a name", n + 1);
                    }
                    if choices.iter().any(|other| other.name == name) {
                        bail!("{what}: duplicate choice {name:?}");
                    }
                    if c.command.is_empty() {
                        bail!("{what}: choice {name:?}: command must not be empty");
                    }
                    choices.push(Choice {
                        name,
                        command: c.command,
                    });
                }
                (Vec::new(), choices)
            }
        };
        if t.stop_command.as_ref().is_some_and(|c| c.is_empty()) {
            bail!("{what}: stop_command must not be empty (leave it out instead)");
        }
        let color = t
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .with_context(|| format!("{what}: color"))?;
        let icon_command = choices.first().map_or(&command, |c| &c.command);
        let icon = resolve_icon(t.icon.as_deref(), icon_command, icon_roots);
        tiles.push(Tile {
            id: t.id,
            name: t.name,
            command,
            choices,
            stop_command: t.stop_command,
            icon,
            color,
        });
    }

    Ok(Config {
        clock,
        home_keys,
        guide_hold: Duration::from_millis(guide_hold_ms),
        tiles,
        mirroring_name,
    })
}

/// `flatpak run` options that take their value as the next argument.
const FLATPAK_VALUE_OPTIONS: &[&str] = &[
    "--arch",
    "--branch",
    "--command",
    "--cwd",
    "--runtime",
    "--runtime-version",
    "--commit",
    "--runtime-commit",
];

/// The app id of a `flatpak run [options] <app-id> [args]` command.
pub fn flatpak_app_id(command: &[String]) -> Option<&str> {
    let program = Path::new(command.first()?).file_name()?;
    if program != "flatpak" || command.get(1).map(String::as_str) != Some("run") {
        return None;
    }
    let mut rest = command[2..].iter();
    while let Some(arg) = rest.next() {
        if FLATPAK_VALUE_OPTIONS.contains(&arg.as_str()) {
            rest.next();
        } else if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

/// Parses `#rrggbb`.
pub fn parse_color(s: &str) -> Result<(u8, u8, u8)> {
    let hex = s
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow::anyhow!("expected \"#rrggbb\", got {s:?}"))?;
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("validated hex");
    Ok((byte(0), byte(2), byte(4)))
}

fn resolve_icon(explicit: Option<&str>, command: &[String], roots: &IconRoots) -> Option<PathBuf> {
    if let Some(icon) = explicit {
        let path = roots.assets.join(icon); // absolute `icon` replaces the base
        if path.is_file() {
            return Some(path);
        }
        tracing::warn!("icon {} not found; falling back", path.display());
    }
    let app_id = flatpak_app_id(command)?;
    roots
        .flatpak_icons
        .iter()
        .find_map(|root| flatpak_icon(root, app_id))
}

fn flatpak_icon(hicolor: &Path, app_id: &str) -> Option<PathBuf> {
    let svg = hicolor.join("scalable/apps").join(format!("{app_id}.svg"));
    if svg.is_file() {
        return Some(svg);
    }
    std::fs::read_dir(hicolor)
        .ok()?
        .filter_map(|entry| {
            let dir = entry.ok()?.file_name().into_string().ok()?;
            let size: u32 = dir.split_once('x')?.0.parse().ok()?;
            let png = hicolor
                .join(&dir)
                .join("apps")
                .join(format!("{app_id}.png"));
            png.is_file().then_some((size, png))
        })
        .max_by_key(|(size, _)| *size)
        .map(|(_, png)| png)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    launcher: RawLauncher,
    #[serde(default, rename = "tile")]
    tiles: Vec<RawTile>,
    #[serde(default)]
    mirroring: RawMirroring,
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMirroring {
    name: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLauncher {
    clock: Option<String>,
    home_keys: Option<Vec<String>>,
    guide_hold_ms: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTile {
    id: String,
    name: String,
    command: Option<Vec<String>>,
    #[serde(default, rename = "choice")]
    choices: Vec<RawChoice>,
    stop_command: Option<Vec<String>>,
    icon: Option<String>,
    color: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChoice {
    name: String,
    command: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn roots(dir: &Path) -> IconRoots {
        IconRoots {
            assets: dir.join("assets"),
            flatpak_icons: vec![dir.join("hicolor")],
        }
    }

    fn no_icons() -> IconRoots {
        IconRoots {
            assets: PathBuf::from("/nonexistent/assets"),
            flatpak_icons: vec![],
        }
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    const MINIMAL: &str = r#"
        [[tile]]
        id = "youtube"
        name = "YouTube"
        command = ["flatpak", "run", "rocks.shy.VacuumTube"]
    "#;

    fn err_of(text: &str) -> String {
        format!("{:#}", parse(text, &no_icons()).unwrap_err())
    }

    #[test]
    fn minimal_config_gets_defaults() {
        let c = parse(MINIMAL, &no_icons()).unwrap();
        assert_eq!(c.clock, ClockStyle::H24);
        assert_eq!(c.home_keys, vec![KeyCode::KEY_HOMEPAGE]);
        assert_eq!(c.guide_hold, Duration::from_millis(1000));
        assert_eq!(c.tiles.len(), 1);
        let t = &c.tiles[0];
        assert_eq!(t.id, "youtube");
        assert_eq!(t.name, "YouTube");
        assert_eq!(t.command, args(&["flatpak", "run", "rocks.shy.VacuumTube"]));
        assert!(t.stop_command.is_none());
        assert!(t.color.is_none());
    }

    #[test]
    fn launcher_section_overrides_defaults() {
        let c = parse(
            &format!(
                "[launcher]\nclock = \"12h\"\nhome_keys = [\"KEY_HOMEPAGE\", \"KEY_BACK\"]\nguide_hold_ms = 1500\n{MINIMAL}"
            ),
            &no_icons(),
        )
        .unwrap();
        assert_eq!(c.clock, ClockStyle::H12);
        assert_eq!(c.home_keys, vec![KeyCode::KEY_HOMEPAGE, KeyCode::KEY_BACK]);
        assert_eq!(c.guide_hold, Duration::from_millis(1500));
    }

    #[test]
    fn tiles_keep_their_order_and_optional_fields() {
        let c = parse(
            r##"
            [[tile]]
            id = "a"
            name = "A"
            command = ["a"]
            color = "#c4302b"
            stop_command = ["stop-a", "now"]
            [[tile]]
            id = "b-2"
            name = "B"
            command = ["b"]
            "##,
            &no_icons(),
        )
        .unwrap();
        let ids: Vec<_> = c.tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b-2"]);
        assert_eq!(c.tiles[0].color, Some((0xc4, 0x30, 0x2b)));
        assert_eq!(c.tiles[0].stop_command, Some(args(&["stop-a", "now"])));
    }

    #[test]
    fn rejects_config_without_tiles() {
        assert!(err_of("[launcher]\nclock = \"24h\"\n").contains("at least one [[tile]]"));
    }

    #[test]
    fn rejects_empty_command() {
        let e = err_of("[[tile]]\nid = \"x\"\nname = \"X\"\ncommand = []\n");
        assert!(e.contains("\"X\"") && e.contains("command"), "{e}");
    }

    #[test]
    fn rejects_empty_stop_command() {
        let e =
            err_of("[[tile]]\nid = \"x\"\nname = \"X\"\ncommand = [\"x\"]\nstop_command = []\n");
        assert!(e.contains("stop_command"), "{e}");
    }

    #[test]
    fn rejects_duplicate_ids() {
        let e = err_of(&format!("{MINIMAL}{MINIMAL}"));
        assert!(e.contains("duplicate") && e.contains("youtube"), "{e}");
    }

    #[test]
    fn rejects_bad_id_characters() {
        let e = err_of("[[tile]]\nid = \"You Tube\"\nname = \"X\"\ncommand = [\"x\"]\n");
        assert!(e.contains("id") && e.contains("You Tube"), "{e}");
    }

    #[test]
    fn rejects_unknown_home_key() {
        let e = err_of(&format!(
            "[launcher]\nhome_keys = [\"KEY_NOPE\"]\n{MINIMAL}"
        ));
        assert!(e.contains("KEY_NOPE"), "{e}");
    }

    #[test]
    fn rejects_empty_home_keys() {
        let e = err_of(&format!("[launcher]\nhome_keys = []\n{MINIMAL}"));
        assert!(e.contains("home_keys"), "{e}");
    }

    #[test]
    fn rejects_guide_hold_out_of_range() {
        for ms in [100, 9000] {
            let e = err_of(&format!("[launcher]\nguide_hold_ms = {ms}\n{MINIMAL}"));
            assert!(e.contains("guide_hold_ms"), "{e}");
        }
    }

    #[test]
    fn rejects_bad_clock_style() {
        let e = err_of(&format!("[launcher]\nclock = \"25h\"\n{MINIMAL}"));
        assert!(e.contains("clock"), "{e}");
    }

    #[test]
    fn rejects_bad_color() {
        let e = err_of("[[tile]]\nid = \"x\"\nname = \"X\"\ncommand = [\"x\"]\ncolor = \"red\"\n");
        assert!(e.contains("color") && e.contains("red"), "{e}");
    }

    #[test]
    fn rejects_unknown_fields() {
        let e = err_of("[[tile]]\nid = \"x\"\nname = \"X\"\ncommand = [\"x\"]\ncomand = [\"y\"]\n");
        assert!(e.contains("comand"), "{e}");
    }

    #[test]
    fn parse_color_accepts_hex() {
        assert_eq!(parse_color("#00ff7f").unwrap(), (0, 255, 127));
        assert_eq!(parse_color("#ABCDEF").unwrap(), (0xab, 0xcd, 0xef));
        assert!(parse_color("00ff7f").is_err());
        assert!(parse_color("#00ff7").is_err());
        assert!(parse_color("#00gg7f").is_err());
    }

    #[test]
    fn flatpak_app_id_finds_the_app() {
        let cmd = args(&["flatpak", "run", "--branch=stable", "a.b.C", "--x"]);
        assert_eq!(flatpak_app_id(&cmd), Some("a.b.C"));
        let cmd = args(&["/usr/bin/flatpak", "run", "--command", "tool", "a.b.C"]);
        assert_eq!(flatpak_app_id(&cmd), Some("a.b.C"));
        assert_eq!(flatpak_app_id(&args(&["mpv", "run", "x"])), None);
        assert_eq!(
            flatpak_app_id(&args(&["flatpak", "install", "a.b.C"])),
            None
        );
        assert_eq!(flatpak_app_id(&args(&["flatpak", "run"])), None);
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn tile_with(command: &str, icon: Option<&str>) -> String {
        let icon = icon
            .map(|i| format!("icon = \"{i}\"\n"))
            .unwrap_or_default();
        format!("[[tile]]\nid = \"t\"\nname = \"T\"\ncommand = {command}\n{icon}")
    }

    #[test]
    fn explicit_relative_icon_resolves_against_assets() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("assets/yt.png"));
        let c = parse(&tile_with("[\"x\"]", Some("yt.png")), &roots(d.path())).unwrap();
        assert_eq!(c.tiles[0].icon, Some(d.path().join("assets/yt.png")));
    }

    #[test]
    fn explicit_icon_wins_over_flatpak_icon() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("assets/yt.png"));
        touch(&d.path().join("hicolor/scalable/apps/a.b.C.svg"));
        let c = parse(
            &tile_with("[\"flatpak\", \"run\", \"a.b.C\"]", Some("yt.png")),
            &roots(d.path()),
        )
        .unwrap();
        assert_eq!(c.tiles[0].icon, Some(d.path().join("assets/yt.png")));
    }

    #[test]
    fn missing_explicit_icon_falls_back_instead_of_failing() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("hicolor/scalable/apps/a.b.C.svg"));
        let c = parse(
            &tile_with("[\"flatpak\", \"run\", \"a.b.C\"]", Some("missing.png")),
            &roots(d.path()),
        )
        .unwrap();
        assert_eq!(
            c.tiles[0].icon,
            Some(d.path().join("hicolor/scalable/apps/a.b.C.svg"))
        );
    }

    #[test]
    fn flatpak_icon_prefers_svg() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("hicolor/256x256/apps/a.b.C.png"));
        touch(&d.path().join("hicolor/scalable/apps/a.b.C.svg"));
        let c = parse(
            &tile_with("[\"flatpak\", \"run\", \"a.b.C\"]", None),
            &roots(d.path()),
        )
        .unwrap();
        assert_eq!(
            c.tiles[0].icon,
            Some(d.path().join("hicolor/scalable/apps/a.b.C.svg"))
        );
    }

    #[test]
    fn flatpak_icon_picks_largest_png() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("hicolor/64x64/apps/a.b.C.png"));
        touch(&d.path().join("hicolor/512x512/apps/a.b.C.png"));
        touch(&d.path().join("hicolor/256x256/apps/a.b.C.png"));
        touch(&d.path().join("hicolor/1024x1024/apps/other.png"));
        let c = parse(
            &tile_with("[\"flatpak\", \"run\", \"a.b.C\"]", None),
            &roots(d.path()),
        )
        .unwrap();
        assert_eq!(
            c.tiles[0].icon,
            Some(d.path().join("hicolor/512x512/apps/a.b.C.png"))
        );
    }

    #[test]
    fn no_icon_found_means_none() {
        let d = tempfile::tempdir().unwrap();
        let c = parse(
            &tile_with("[\"flatpak\", \"run\", \"a.b.C\"]", None),
            &roots(d.path()),
        )
        .unwrap();
        assert_eq!(c.tiles[0].icon, None);
        let c = parse(&tile_with("[\"mpv\"]", None), &roots(d.path())).unwrap();
        assert_eq!(c.tiles[0].icon, None);
    }

    #[test]
    fn load_reports_the_path() {
        let e = format!(
            "{:#}",
            load(Path::new("/nonexistent/launcher.toml"), &no_icons()).unwrap_err()
        );
        assert!(e.contains("/nonexistent/launcher.toml"), "{e}");
    }

    #[test]
    fn mirroring_name_defaults_to_mushtv() {
        assert_eq!(
            parse(MINIMAL, &no_icons()).unwrap().mirroring_name,
            "mushtv"
        );
    }

    #[test]
    fn mirroring_name_can_be_set() {
        let c = parse(
            &format!("[mirroring]\nname = \"Living room\"\n{MINIMAL}"),
            &no_icons(),
        )
        .unwrap();
        assert_eq!(c.mirroring_name, "Living room");
    }

    #[test]
    fn empty_mirroring_name_is_rejected() {
        let e = err_of(&format!("[mirroring]\nname = \"  \"\n{MINIMAL}"));
        assert!(e.contains("[mirroring] name"), "{e}");
    }

    #[test]
    fn repo_config_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("launcher.toml");
        load(&path, &no_icons()).unwrap();
    }

    const CHOICES: &str = r#"
        [[tile]]
        id = "steam"
        name = "Steam"
        [[tile.choice]]
        name = "Graphics card (RTX 4060)"
        command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc", "Steam"]
        [[tile.choice]]
        name = "Integrated graphics card"
        command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc-igpu", "Steam"]
    "#;

    #[test]
    fn tile_choices_parse_in_order() {
        let c = parse(CHOICES, &no_icons()).unwrap();
        let t = &c.tiles[0];
        assert!(t.command.is_empty());
        let names: Vec<_> = t.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Graphics card (RTX 4060)", "Integrated graphics card"]
        );
        assert_eq!(t.choices[1].command[4], "spc-igpu");
    }

    #[test]
    fn plain_tiles_have_no_choices() {
        assert!(
            parse(MINIMAL, &no_icons()).unwrap().tiles[0]
                .choices
                .is_empty()
        );
    }

    #[test]
    fn rejects_command_and_choices_together() {
        let text = CHOICES.replace("name = \"Steam\"", "name = \"Steam\"\ncommand = [\"x\"]");
        let e = err_of(&text);
        assert!(e.contains("either command or [[tile.choice]]"), "{e}");
    }

    #[test]
    fn rejects_a_tile_without_command_or_choices() {
        let e = err_of("[[tile]]\nid = \"a\"\nname = \"A\"\n");
        assert!(e.contains("needs a command"), "{e}");
    }

    #[test]
    fn rejects_a_single_choice() {
        let e = err_of(
            "[[tile]]\nid = \"a\"\nname = \"A\"\n[[tile.choice]]\nname = \"One\"\ncommand = [\"x\"]\n",
        );
        assert!(e.contains("two or more"), "{e}");
    }

    #[test]
    fn rejects_bad_choices() {
        let blank = CHOICES.replace("name = \"Integrated graphics card\"", "name = \"  \"");
        let e = err_of(&blank);
        assert!(e.contains("needs a name"), "{e}");
        let dup = CHOICES.replace("Integrated graphics card", "Graphics card (RTX 4060)");
        let e = err_of(&dup);
        assert!(e.contains("duplicate choice"), "{e}");
        let empty = CHOICES.replace(
            r#"command = ["flatpak", "run", "com.moonlight_stream.Moonlight", "stream", "spc-igpu", "Steam"]"#,
            "command = []",
        );
        let e = err_of(&empty);
        assert!(e.contains("command must not be empty"), "{e}");
    }

    #[test]
    fn a_tile_with_choices_takes_its_icon_from_the_first_choice() {
        let d = tempfile::tempdir().unwrap();
        let svg = d
            .path()
            .join("hicolor/scalable/apps/com.moonlight_stream.Moonlight.svg");
        touch(&svg);
        let c = parse(CHOICES, &roots(d.path())).unwrap();
        assert_eq!(c.tiles[0].icon.as_deref(), Some(svg.as_path()));
    }
}
