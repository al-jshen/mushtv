mod audio;
mod bluetooth;
mod config;
mod input;
mod mirroring;
mod mirroring_ui;
mod prefs;
mod runner;
mod settings;
mod settings_ui;

slint::include_modules!();

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use config::{ClockStyle, Config, IconRoots, Tile};
use input::{Action, InputHandle};
use runner::{App, RunEvent, Runner};

const DEFAULT_CONFIG: &str = "/etc/mushtv/launcher.toml";
const ASSETS_DIR: &str = "/usr/local/share/mushtv";
const FLATPAK_ICONS: &[&str] = &["/var/lib/flatpak/exports/share/icons/hicolor"];
const TOAST_TIME: Duration = Duration::from_secs(5);
const RETURN_LOCK: Duration = input::logic::RETURN_COOLDOWN;
/// How long shutdown waits for the running app to exit.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(7);

/// Tile backgrounds for tiles without a configured color.
const PALETTE: &[(u8, u8, u8)] = &[
    (0x1f, 0x2a, 0x44),
    (0x2a, 0x1f, 0x3d),
    (0x13, 0x33, 0x2e),
    (0x3d, 0x24, 0x1f),
    (0x1c, 0x2f, 0x3f),
    (0x33, 0x2d, 0x16),
];

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run { config: PathBuf },
    CheckConfig { config: PathBuf },
    PrintInput { config: PathBuf },
}

fn parse_args(args: &[String]) -> Result<Cli> {
    let mut config = PathBuf::from(DEFAULT_CONFIG);
    let mut check = None;
    let mut print_input = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => config = it.next().context("--config needs a path")?.into(),
            "--check-config" => {
                check = Some(PathBuf::from(
                    it.next().context("--check-config needs a path")?,
                ))
            }
            "--print-input" => print_input = true,
            other => bail!(
                "unknown argument {other:?} (expected --config, --check-config or --print-input)"
            ),
        }
    }
    match (check, print_input) {
        (Some(_), true) => bail!("--check-config and --print-input can't be combined"),
        (Some(config), false) => Ok(Cli::CheckConfig { config }),
        (None, true) => Ok(Cli::PrintInput { config }),
        (None, false) => Ok(Cli::Run { config }),
    }
}

/// Time and date lines for the clock.
fn format_clock<Tz: TimeZone>(now: &DateTime<Tz>, style: ClockStyle) -> (String, String)
where
    Tz::Offset: std::fmt::Display,
{
    let time = match style {
        ClockStyle::H24 => now.format("%H:%M"),
        ClockStyle::H12 => now.format("%-I:%M %p"),
    };
    (time.to_string(), now.format("%a %-d %b").to_string())
}

/// The "cast" glyph (Material Icons) for the built-in Mirroring tile.
const MIRRORING_GLYPH: &str = "\u{e307}";

/// The toast when an app fails to start or dies right away.
fn failure_toast(app: App, name: &str, spawn_failed: bool) -> String {
    match (app, spawn_failed) {
        (App::Mirroring, _) => "Couldn't start mirroring".to_string(),
        (App::Tile(_), true) => format!("Couldn't open {name}"),
        (App::Tile(_), false) => format!("{name} closed unexpectedly"),
    }
}

/// Tile background: the configured color, else one from a fixed palette.
fn tile_color(index: usize, configured: Option<(u8, u8, u8)>) -> (u8, u8, u8) {
    configured.unwrap_or(PALETTE[index % PALETTE.len()])
}

/// `background.png` or `background.jpg` in the assets directory, if present.
fn background_path(assets: &Path) -> Option<PathBuf> {
    ["background.png", "background.jpg"]
        .iter()
        .map(|name| assets.join(name))
        .find(|p| p.is_file())
}

fn main() {
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|cli| match cli {
        Cli::CheckConfig { config } => check_config(&config),
        Cli::PrintInput { config } => {
            print_input(&config);
            Ok(())
        }
        Cli::Run { config } => run(&config),
    });
    if let Err(e) = result {
        tracing::error!("{e:#}");
        eprintln!("mushtv-launcher: {e:#}");
        std::process::exit(1);
    }
}

/// Our own logs from info up; everything else (winit, calloop, …) only from warn.
fn log_filter() -> tracing_subscriber::filter::Targets {
    tracing_subscriber::filter::Targets::new()
        .with_target("mushtv_launcher", tracing::Level::INFO)
        .with_default(tracing::Level::WARN)
}

fn init_logging() {
    use tracing_subscriber::prelude::*;
    let journal = Path::new("/run/systemd/journal/socket")
        .exists()
        .then(|| tracing_journald::layer().ok())
        .flatten()
        .map(|layer| layer.with_syslog_identifier("mushtv-launcher".into()));
    let stderr = journal
        .is_none()
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry()
        .with(journal)
        .with(stderr)
        .with(log_filter())
        .init();
}

fn icon_roots() -> IconRoots {
    IconRoots {
        assets: PathBuf::from(ASSETS_DIR),
        flatpak_icons: FLATPAK_ICONS.iter().map(PathBuf::from).collect(),
    }
}

fn check_config(path: &Path) -> Result<()> {
    let config = config::load(path, &icon_roots())?;
    println!("{}: OK, {} tiles", path.display(), config.tiles.len());
    Ok(())
}

fn print_input(path: &Path) {
    let home_keys = match config::load(path, &icon_roots()) {
        Ok(c) => c.home_keys,
        Err(e) => {
            eprintln!("({e:#}; using the default home key)");
            vec![evdev::KeyCode::KEY_HOMEPAGE]
        }
    };
    for line in input::describe_devices(&home_keys) {
        println!("{line}");
    }
}

fn run(config_path: &Path) -> Result<()> {
    let ui = MainWindow::new()?;
    let config = match config::load(config_path, &icon_roots()) {
        Ok(config) => {
            tracing::info!(
                "loaded {} with {} tiles",
                config_path.display(),
                config.tiles.len()
            );
            config
        }
        Err(e) => {
            tracing::error!("{e:#}");
            ui.set_error_title("The launcher's config has a problem".into());
            ui.set_error_details(format!("{e:#}").into());
            Config {
                clock: ClockStyle::H24,
                home_keys: vec![evdev::KeyCode::KEY_HOMEPAGE],
                guide_hold: Duration::from_millis(1000),
                tiles: vec![],
                mirroring_name: "mushtv".to_string(),
            }
        }
    };

    let mut tile_data = tile_models(&config.tiles);
    if ui.get_error_title().is_empty() {
        tile_data.push(TileData {
            name: "Mirroring".into(),
            icon: slint::Image::default(),
            has_icon: false,
            color: slint::Color::from_rgb_u8(0x24, 0x2c, 0x3e),
            glyph: MIRRORING_GLYPH.into(),
        });
        tile_data.push(TileData {
            name: "Settings".into(),
            icon: slint::Image::default(),
            has_icon: false,
            color: slint::Color::from_rgb_u8(0x2b, 0x31, 0x3c),
            glyph: settings_ui::SETTINGS_GLYPH.into(),
        });
    }
    ui.set_tiles(ModelRc::from(Rc::new(VecModel::from(tile_data))));
    let prefs_path = prefs::default_path();
    let prefs = prefs_path.as_deref().map(prefs::load).unwrap_or_default();
    settings_ui::init(&ui, prefs_path, prefs);
    if let Some(bg) = background_path(Path::new(ASSETS_DIR)) {
        match slint::Image::load_from_path(&bg) {
            Ok(image) => {
                ui.set_background_image(image);
                ui.set_has_background_image(true);
            }
            Err(e) => tracing::warn!("couldn't load {}: {e}", bg.display()),
        }
    }

    let clock_style = config.clock;
    let update_clock = {
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                let (time, date) = format_clock(&chrono::Local::now(), clock_style);
                ui.set_clock_time(time.into());
                ui.set_clock_date(date.into());
            }
        }
    };
    update_clock();
    let clock = Timer::default();
    clock.start(TimerMode::Repeated, Duration::from_secs(1), update_clock);

    let tiles = Arc::new(config.tiles);
    let journal = Path::new("/run/systemd/journal/socket").exists();

    // The input thread needs the runner (for Home), and the runner's events
    // need the input handle (to end the "app active" state). Break the cycle
    // with a slot that is filled once both exist.
    let input_slot: Arc<std::sync::OnceLock<InputHandle>> = Arc::default();

    let runner = Arc::new(Runner::new(
        {
            let weak = ui.as_weak();
            let tiles = Arc::clone(&tiles);
            let input_slot = Arc::clone(&input_slot);
            move |event| {
                let tiles = Arc::clone(&tiles);
                let input_slot = Arc::clone(&input_slot);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    on_run_event(&ui, &tiles, input_slot.get(), event)
                });
            }
        },
        journal,
    ));

    let input = input::spawn(config.home_keys.clone(), config.guide_hold, {
        let weak = ui.as_weak();
        let runner = Arc::clone(&runner);
        move |action| {
            let runner = Arc::clone(&runner);
            let _ = weak.upgrade_in_event_loop(move |ui| on_action(&ui, &runner, action));
        }
    });
    let _ = input_slot.set(input.clone());
    mirroring_ui::init(
        &ui,
        Arc::clone(&runner),
        Arc::clone(&input_slot),
        config.mirroring_name.clone(),
    );

    ui.on_activate({
        let weak = ui.as_weak();
        let runner = Arc::clone(&runner);
        let tiles = Arc::clone(&tiles);
        move |index| {
            let (Some(ui), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                return;
            };
            if settings_ui::is_open() || mirroring_ui::is_open() {
                return; // a hidden tile under the pointer
            }
            // Built-in tiles come after the configured apps.
            if index == tiles.len() {
                mirroring_ui::open();
                return;
            }
            if index == tiles.len() + 1 {
                settings_ui::open();
                return;
            }
            let Some(tile) = tiles.get(index) else { return };
            if runner.launch(index, tile) {
                tracing::info!("opening {} ({})", tile.name, tile.command.join(" "));
                ui.set_busy_text(format!("Opening {}…", tile.name).into());
                input.set_app_active(true);
            }
        }
    });

    // SIGTERM/SIGINT (e.g. the TV session restarting): stop the app first.
    {
        let runner = Arc::clone(&runner);
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ])?;
        std::thread::spawn(move || {
            if let Some(sig) = signals.forever().next() {
                tracing::info!("signal {sig}: stopping the running app and exiting");
                runner.stop_blocking(SHUTDOWN_WAIT);
                let _ = slint::quit_event_loop();
            }
        });
    }

    let result = ui.run().map_err(anyhow::Error::from);
    finish(result, &runner)
}

/// Ends a run: the app is stopped whether the UI loop ended cleanly or with an
/// error (e.g. cage went away first), and only then is the result returned.
fn finish(result: Result<()>, runner: &Runner) -> Result<()> {
    runner.stop_blocking(SHUTDOWN_WAIT);
    result
}

fn tile_models(tiles: &[Tile]) -> Vec<TileData> {
    tiles
        .iter()
        .enumerate()
        .map(|(i, tile)| {
            let icon =
                tile.icon
                    .as_ref()
                    .and_then(|path| match slint::Image::load_from_path(path) {
                        Ok(image) => Some(image),
                        Err(e) => {
                            tracing::warn!("couldn't load icon {}: {e}", path.display());
                            None
                        }
                    });
            let (r, g, b) = tile_color(i, tile.color);
            TileData {
                name: tile.name.as_str().into(),
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
                color: slint::Color::from_rgb_u8(r, g, b),
                glyph: SharedString::new(),
            }
        })
        .collect()
}

fn on_run_event(ui: &MainWindow, tiles: &[Tile], input: Option<&InputHandle>, event: RunEvent) {
    let event_app = match &event {
        RunEvent::Exited { app, .. } | RunEvent::SpawnError { app, .. } => *app,
    };
    ui.set_busy_text(SharedString::new());
    if let Some(input) = input {
        input.set_app_active(false);
    }
    // Swallow a still-held key or button for a moment after coming back.
    ui.set_input_locked(true);
    let weak = ui.as_weak();
    Timer::single_shot(RETURN_LOCK, move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_input_locked(false);
        }
    });

    let name = |app: App| match app {
        App::Tile(i) => tiles.get(i).map_or("the app", |t| t.name.as_str()),
        App::Mirroring => "Mirroring",
    };
    match event {
        RunEvent::Exited { app, code, quick } => {
            tracing::info!("{} exited (code {code:?})", name(app));
            if quick {
                show_toast(ui, failure_toast(app, name(app), false));
            }
        }
        RunEvent::SpawnError { app, error } => {
            tracing::error!("couldn't open {}: {error}", name(app));
            show_toast(ui, failure_toast(app, name(app), true));
        }
    }
    if event_app == App::Mirroring {
        mirroring_ui::on_exit();
    }
}

fn on_action(ui: &MainWindow, runner: &Runner, action: Action) {
    if mirroring_ui::is_open() {
        match action {
            Action::Back => mirroring_ui::back(),
            Action::Home => {
                tracing::info!("home: stopping mirroring");
                runner.stop();
            }
            _ => {}
        }
        return;
    }
    if settings_ui::is_open() {
        let nav = match action {
            Action::Up => settings::Nav::Up,
            Action::Down => settings::Nav::Down,
            Action::Activate => settings::Nav::Select,
            Action::Back => settings::Nav::Back,
            Action::Home => settings::Nav::Home,
            Action::Left | Action::Right => return,
        };
        settings_ui::nav_to(nav);
        return;
    }
    match action {
        Action::Left => ui.invoke_move_focus(-1),
        Action::Right => ui.invoke_move_focus(1),
        Action::Up => ui.invoke_move_row(-1),
        Action::Down => ui.invoke_move_row(1),
        Action::Back => {}
        Action::Activate => ui.invoke_activate_focused(),
        Action::Home => {
            if runner.is_active() {
                tracing::info!("home: stopping the running app");
                runner.stop();
            }
        }
    }
}

fn show_toast(ui: &MainWindow, text: String) {
    let text = SharedString::from(text);
    ui.set_toast_text(text.clone());
    let weak = ui.as_weak();
    Timer::single_shot(TOAST_TIME, move || {
        if let Some(ui) = weak.upgrade().filter(|ui| ui.get_toast_text() == text) {
            ui.set_toast_text(SharedString::new());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_runs_with_the_default_config() {
        assert_eq!(
            parse_args(&args(&[])).unwrap(),
            Cli::Run {
                config: PathBuf::from("/etc/mushtv/launcher.toml")
            }
        );
    }

    #[test]
    fn config_flag_sets_the_path() {
        assert_eq!(
            parse_args(&args(&["--config", "/tmp/x.toml"])).unwrap(),
            Cli::Run {
                config: PathBuf::from("/tmp/x.toml")
            }
        );
    }

    #[test]
    fn check_config_takes_a_path() {
        assert_eq!(
            parse_args(&args(&["--check-config", "launcher.toml"])).unwrap(),
            Cli::CheckConfig {
                config: PathBuf::from("launcher.toml")
            }
        );
    }

    #[test]
    fn print_input_uses_the_config_for_home_keys() {
        assert_eq!(
            parse_args(&args(&["--print-input", "--config", "/tmp/x.toml"])).unwrap(),
            Cli::PrintInput {
                config: PathBuf::from("/tmp/x.toml")
            }
        );
    }

    #[test]
    fn bad_args_are_errors() {
        assert!(parse_args(&args(&["--config"])).is_err());
        assert!(parse_args(&args(&["--check-config"])).is_err());
        assert!(parse_args(&args(&["--frobnicate"])).is_err());
        assert!(parse_args(&args(&["--check-config", "a", "--print-input"])).is_err());
    }

    #[test]
    fn clock_formats_both_styles() {
        let t = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 13, 21, 5, 0)
            .unwrap();
        assert_eq!(
            format_clock(&t, ClockStyle::H24),
            ("21:05".into(), "Tue 13 Oct".into())
        );
        assert_eq!(
            format_clock(&t, ClockStyle::H12),
            ("9:05 PM".into(), "Tue 13 Oct".into())
        );
        let morning = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 2, 7, 30, 0)
            .unwrap();
        assert_eq!(
            format_clock(&morning, ClockStyle::H24),
            ("07:30".into(), "Fri 2 Jan".into())
        );
    }

    #[test]
    fn configured_tile_color_wins() {
        assert_eq!(tile_color(0, Some((1, 2, 3))), (1, 2, 3));
    }

    #[test]
    fn palette_colors_differ_for_neighbours() {
        let a = tile_color(0, None);
        let b = tile_color(1, None);
        assert_ne!(a, b);
        assert_ne!(a, (0, 0, 0));
    }

    #[test]
    fn log_filter_keeps_our_info_and_drops_library_debug() {
        use tracing::Level;
        let f = log_filter();
        assert!(f.would_enable("mushtv_launcher", &Level::INFO));
        assert!(f.would_enable("mushtv_launcher::runner", &Level::WARN));
        assert!(!f.would_enable("mushtv_launcher", &Level::DEBUG));
        assert!(!f.would_enable("calloop", &Level::DEBUG));
        assert!(!f.would_enable("winit::platform_impl", &Level::INFO));
        assert!(f.would_enable("winit::platform_impl", &Level::WARN));
    }

    #[test]
    fn finish_stops_the_app_even_when_the_ui_loop_failed() {
        let runner = Runner::new(|_| {}, false);
        let tile = Tile {
            id: "t".into(),
            name: "T".into(),
            command: args(&["sleep", "30"]),
            stop_command: None,
            icon: None,
            color: None,
        };
        assert!(runner.launch(0, &tile));
        let result = finish(Err(anyhow::anyhow!("wayland connection lost")), &runner);
        assert!(result.is_err());
        assert!(!runner.is_active(), "the app was left running");
    }

    #[test]
    fn failure_toasts() {
        assert_eq!(
            failure_toast(App::Tile(0), "YouTube", true),
            "Couldn't open YouTube"
        );
        assert_eq!(
            failure_toast(App::Tile(0), "YouTube", false),
            "YouTube closed unexpectedly"
        );
        assert_eq!(
            failure_toast(App::Mirroring, "Mirroring", true),
            "Couldn't start mirroring"
        );
        assert_eq!(
            failure_toast(App::Mirroring, "Mirroring", false),
            "Couldn't start mirroring"
        );
    }

    #[test]
    fn background_prefers_png_then_jpg() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(background_path(d.path()), None);
        std::fs::write(d.path().join("background.jpg"), b"x").unwrap();
        assert_eq!(
            background_path(d.path()),
            Some(d.path().join("background.jpg"))
        );
        std::fs::write(d.path().join("background.png"), b"x").unwrap();
        assert_eq!(
            background_path(d.path()),
            Some(d.path().join("background.png"))
        );
    }
}
