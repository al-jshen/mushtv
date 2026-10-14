//! Reads gamepads and Home keys straight from /dev/input, so they work no
//! matter which app is in front. Devices are never grabbed: apps still get
//! every event.

pub mod logic;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evdev::{AbsInfo, AbsoluteAxisCode, Device, EventSummary, KeyCode};
use inotify::{Inotify, WatchMask};

pub use logic::Action;
use logic::{Axis, DevId, Logic, RawEvent};

const INPUT_DIR: &str = "/dev/input";

enum Msg {
    Raw(RawEvent),
    AppActive(bool, Instant),
}

/// Lets the launcher tell the input thread whether an app is in front.
#[derive(Clone)]
pub struct InputHandle {
    tx: Sender<Msg>,
}

impl InputHandle {
    /// True while an app is opening, running or stopping.
    pub fn set_app_active(&self, active: bool) {
        let _ = self.tx.send(Msg::AppActive(active, Instant::now()));
    }
}

/// What a device is useful for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DevKind {
    pub gamepad: bool,
    pub home_source: bool,
    /// Has a remote-style OK/Select key.
    pub select_source: bool,
    /// Has a remote-style Back/Exit key.
    pub back_source: bool,
}

/// Remote OK/Select keys. They have no keysym, so Wayland apps (including
/// the launcher's UI) never see them; the launcher reads them here instead.
const SELECT_KEYS: [KeyCode; 2] = [KeyCode::KEY_OK, KeyCode::KEY_SELECT];

/// Remote Back/Exit keys. Like OK/Select, apps can't rely on these through
/// Wayland, so the launcher reads them here. (Escape and Backspace arrive
/// through Wayland as normal keys.)
const BACK_KEYS: [KeyCode; 2] = [KeyCode::KEY_BACK, KeyCode::KEY_EXIT];

/// Starts the input threads. `on_action` runs on the input thread.
pub fn spawn(
    home_keys: Vec<KeyCode>,
    guide_hold: Duration,
    on_action: impl Fn(Action) + Send + 'static,
) -> InputHandle {
    let (tx, rx) = mpsc::channel::<Msg>();

    std::thread::Builder::new()
        .name("input-logic".into())
        .spawn(move || {
            let mut logic = Logic::new(guide_hold);
            loop {
                let msg = match logic.next_deadline() {
                    Some(deadline) => {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(msg) => Some(msg),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    None => match rx.recv() {
                        Ok(msg) => Some(msg),
                        Err(_) => return,
                    },
                };
                let now = Instant::now();
                let mut actions = match msg {
                    Some(Msg::Raw(ev)) => logic.on_event(ev, now),
                    Some(Msg::AppActive(active, at)) => {
                        logic.set_app_active(active, at);
                        vec![]
                    }
                    None => vec![],
                };
                actions.extend(logic.on_tick(now));
                for action in actions {
                    on_action(action);
                }
            }
        })
        .expect("spawn input-logic thread");

    let watcher_tx = tx.clone();
    std::thread::Builder::new()
        .name("input-hotplug".into())
        .spawn(move || watch_devices(home_keys, watcher_tx))
        .expect("spawn input-hotplug thread");

    InputHandle { tx }
}

/// Opens every useful device now and whenever /dev/input changes.
fn watch_devices(home_keys: Vec<KeyCode>, tx: Sender<Msg>) {
    let known: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    let next_id = Arc::new(AtomicU32::new(1));

    let mut inotify = match Inotify::init().and_then(|i| {
        i.watches()
            .add(
                INPUT_DIR,
                WatchMask::CREATE | WatchMask::ATTRIB | WatchMask::DELETE,
            )
            .map(|_| i)
    }) {
        Ok(i) => Some(i),
        Err(e) => {
            tracing::warn!("can't watch {INPUT_DIR} for new devices: {e}");
            None
        }
    };

    let mut buffer = [0u8; 4096];
    loop {
        let present = list_event_nodes(Path::new(INPUT_DIR));
        let (added, _) = diff_devices(&known.lock().unwrap(), &present);
        for path in added {
            open_device(&path, &home_keys, &known, &next_id, &tx);
        }
        let Some(inotify) = inotify.as_mut() else {
            return;
        };
        if let Err(e) = inotify.read_events_blocking(&mut buffer) {
            tracing::warn!("input hotplug watch failed: {e}");
            return;
        }
    }
}

fn list_event_nodes(dir: &Path) -> HashSet<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

fn open_device(
    path: &Path,
    home_keys: &[KeyCode],
    known: &Arc<Mutex<HashSet<PathBuf>>>,
    next_id: &AtomicU32,
    tx: &Sender<Msg>,
) {
    // Permission errors are normal right after hotplug (udev hasn't applied
    // the group yet); the ATTRIB event that follows triggers a retry.
    let Ok(device) = Device::open(path) else {
        return;
    };
    let kind = classify(&device, home_keys);
    if kind == DevKind::default() {
        return;
    }
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let name = device.name().unwrap_or("unnamed").to_string();
    tracing::info!("input: using {} ({name}) as {kind:?}", path.display());
    known.lock().unwrap().insert(path.to_path_buf());

    let path = path.to_path_buf();
    let home_keys = home_keys.to_vec();
    let known = Arc::clone(known);
    let tx = tx.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("input-{id}"))
        .spawn(move || {
            let err = read_device(device, id, kind, &home_keys, &tx);
            tracing::info!("input: {} ({name}) gone: {err}", path.display());
            let _ = tx.send(Msg::Raw(RawEvent::DeviceGone(id)));
            known.lock().unwrap().remove(&path);
        });
    if let Err(e) = spawned {
        tracing::warn!("input: couldn't start reader thread: {e}");
    }
}

pub(crate) fn classify(device: &Device, home_keys: &[KeyCode]) -> DevKind {
    let keys = device.supported_keys();
    let has = |k: KeyCode| keys.is_some_and(|keys| keys.contains(k));
    DevKind {
        gamepad: has(KeyCode::BTN_SOUTH),
        home_source: home_keys.iter().any(|&k| has(k)),
        select_source: SELECT_KEYS.iter().any(|&k| has(k)),
        back_source: BACK_KEYS.iter().any(|&k| has(k)),
    }
}

/// Reads one device until it fails (usually: unplugged). Returns the error.
fn read_device(
    mut device: Device,
    id: DevId,
    kind: DevKind,
    home_keys: &[KeyCode],
    tx: &Sender<Msg>,
) -> std::io::Error {
    let abs_info: HashMap<AbsoluteAxisCode, AbsInfo> = device
        .get_absinfo()
        .map(|it| it.collect())
        .unwrap_or_default();
    // Last direction sent per axis source, so a moving stick only sends changes.
    let mut last: HashMap<AbsoluteAxisCode, i8> = HashMap::new();
    let send = |ev: RawEvent| {
        let _ = tx.send(Msg::Raw(ev));
    };

    loop {
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(e) => return e,
        };
        for event in events {
            match event.destructure() {
                EventSummary::Key(_, code, value) => {
                    if value == 2 {
                        continue; // keyboard autorepeat
                    }
                    let down = value == 1;
                    if kind.home_source && down && home_keys.contains(&code) {
                        send(RawEvent::HomeKey);
                    }
                    if kind.select_source && down && SELECT_KEYS.contains(&code) {
                        send(RawEvent::SelectKey);
                    }
                    if kind.back_source && down && BACK_KEYS.contains(&code) {
                        send(RawEvent::Back);
                    }
                    if !kind.gamepad {
                        continue;
                    }
                    let dir = |axis, v| RawEvent::Dir {
                        dev: id,
                        axis,
                        value: if down { v } else { 0 },
                    };
                    match code {
                        KeyCode::BTN_MODE if down => send(RawEvent::GuideDown(id)),
                        KeyCode::BTN_MODE => send(RawEvent::GuideUp(id)),
                        KeyCode::BTN_SOUTH if down => send(RawEvent::South(id)),
                        KeyCode::BTN_EAST if down => send(RawEvent::Back),
                        KeyCode::BTN_DPAD_LEFT => send(dir(Axis::X, -1)),
                        KeyCode::BTN_DPAD_RIGHT => send(dir(Axis::X, 1)),
                        KeyCode::BTN_DPAD_UP => send(dir(Axis::Y, -1)),
                        KeyCode::BTN_DPAD_DOWN => send(dir(Axis::Y, 1)),
                        _ => {}
                    }
                }
                EventSummary::AbsoluteAxis(_, code, value) if kind.gamepad => {
                    let axis = match code {
                        AbsoluteAxisCode::ABS_X | AbsoluteAxisCode::ABS_HAT0X => Axis::X,
                        AbsoluteAxisCode::ABS_Y | AbsoluteAxisCode::ABS_HAT0Y => Axis::Y,
                        _ => continue,
                    };
                    let Some(info) = abs_info.get(&code) else {
                        continue;
                    };
                    let d = axis_to_dir(value, *info);
                    if last.insert(code, d) != Some(d) {
                        send(RawEvent::Dir {
                            dev: id,
                            axis,
                            value: d,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

/// Lists the devices the launcher would use, for `--print-input`.
pub fn describe_devices(home_keys: &[KeyCode]) -> Vec<String> {
    let mut nodes: Vec<_> = list_event_nodes(Path::new(INPUT_DIR)).into_iter().collect();
    nodes.sort();
    nodes
        .into_iter()
        .map(|path| match Device::open(&path) {
            Ok(dev) => format!(
                "{}: {:?} {:?}",
                path.display(),
                dev.name().unwrap_or("unnamed"),
                classify(&dev, home_keys)
            ),
            Err(e) => format!("{}: can't open ({e})", path.display()),
        })
        .collect()
}

pub(crate) fn diff_devices(
    known: &HashSet<PathBuf>,
    present: &HashSet<PathBuf>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut added: Vec<_> = present.difference(known).cloned().collect();
    let mut removed: Vec<_> = known.difference(present).cloned().collect();
    added.sort();
    removed.sort();
    (added, removed)
}

/// Maps an absolute axis value to -1/0/1 with a 50 % deadzone.
pub(crate) fn axis_to_dir(value: i32, info: AbsInfo) -> i8 {
    let (min, max) = (info.minimum() as f64, info.maximum() as f64);
    let half = (max - min) / 2.0;
    if half <= 0.0 {
        return 0;
    }
    let d = (value as f64 - (min + half)) / half;
    if d <= -0.5 {
        -1
    } else if d >= 0.5 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn diff_finds_added_and_removed() {
        let known = set(&["/dev/input/event1", "/dev/input/event2"]);
        let present = set(&["/dev/input/event2", "/dev/input/event7"]);
        let (added, removed) = diff_devices(&known, &present);
        assert_eq!(added, vec![PathBuf::from("/dev/input/event7")]);
        assert_eq!(removed, vec![PathBuf::from("/dev/input/event1")]);
    }

    #[test]
    fn diff_is_sorted_and_empty_when_unchanged() {
        let known = set(&["/dev/input/event1"]);
        assert_eq!(diff_devices(&known, &known), (vec![], vec![]));
        let (added, _) = diff_devices(&set(&[]), &set(&["/dev/input/event9", "/dev/input/event3"]));
        assert_eq!(
            added,
            vec![
                PathBuf::from("/dev/input/event3"),
                PathBuf::from("/dev/input/event9")
            ]
        );
    }

    fn info(min: i32, max: i32) -> AbsInfo {
        AbsInfo::new(0, min, max, 0, 0, 0)
    }

    #[test]
    fn signed_stick_uses_half_range_deadzone() {
        let i = info(-32768, 32767);
        assert_eq!(axis_to_dir(-32768, i), -1);
        assert_eq!(axis_to_dir(32767, i), 1);
        assert_eq!(axis_to_dir(0, i), 0);
        assert_eq!(axis_to_dir(16383, i), 0);
        assert_eq!(axis_to_dir(16384, i), 1);
        assert_eq!(axis_to_dir(-16384, i), 0);
        assert_eq!(axis_to_dir(-16385, i), -1);
    }

    #[test]
    fn unsigned_stick_is_centered() {
        let i = info(0, 255);
        assert_eq!(axis_to_dir(0, i), -1);
        assert_eq!(axis_to_dir(128, i), 0);
        assert_eq!(axis_to_dir(255, i), 1);
        assert_eq!(axis_to_dir(63, i), -1);
        assert_eq!(axis_to_dir(64, i), 0);
    }

    #[test]
    fn hat_passes_through() {
        let i = info(-1, 1);
        assert_eq!(axis_to_dir(-1, i), -1);
        assert_eq!(axis_to_dir(0, i), 0);
        assert_eq!(axis_to_dir(1, i), 1);
    }

    #[test]
    fn degenerate_range_is_neutral() {
        assert_eq!(axis_to_dir(5, info(5, 5)), 0);
    }
}
