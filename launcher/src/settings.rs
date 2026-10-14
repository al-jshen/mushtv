//! The Settings screens as a pure state machine: a page stack with focus,
//! turning keys and fresh data into effects (Bluetooth commands, audio
//! switches, toasts) and a view model for the UI to draw.

use std::time::{Duration, Instant};

use crate::audio::{AudioState, OutputKind, bt_address_from_node_name};
pub use crate::bluetooth::model::BtOp;
use crate::bluetooth::model::{BtDevice, DeviceKind};
use crate::mirroring::MirrorMode;

/// How long to wait for a speaker's audio output after asking to connect it.
pub const CONNECT_SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

/// A Bluetooth operation that never reports back stops blocking new ones
/// after this long (BlueZ's own pairing timeout is shorter).
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(90);

/// Everything the screens show, refreshed by the Bluetooth and audio workers.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub bt_available: bool,
    pub powered: bool,
    pub scanning: bool,
    pub devices: Vec<BtDevice>,
    pub audio: Option<AudioState>,
    /// A passkey to show while pairing (keyboards).
    pub passkey: Option<String>,
    pub mirroring_mode: MirrorMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Select,
    Back,
    Home,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave Settings and show the tiles.
    Close,
    /// Start/stop the Bluetooth worker's polling.
    SetBtActive(bool),
    PowerOn,
    StartScan,
    StopScan,
    Pair(String),
    Connect(String),
    Disconnect(String),
    Forget(String),
    /// Make this PipeWire sink the chosen output. With a `fallback`, set
    /// that first: WirePlumber falls back to the most recently chosen
    /// available output, so this keeps the TV right behind a speaker.
    SetOutput {
        id: u32,
        fallback: Option<u32>,
    },
    Toast(String),
    /// Save and use this mirroring mode.
    SetMirroringMode(MirrorMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    AudioOutput,
    Bluetooth,
    Tv,
    Speaker,
    Gamepad,
    Keyboard,
    Mouse,
    Device,
    Add,
    Connect,
    Disconnect,
    Forget,
    Mirroring,
    LowLag,
    Smooth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// Stable identity (device path or action), so focus can follow a row
    /// when the list changes under it.
    pub key: String,
    pub icon: Icon,
    pub label: String,
    pub detail: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub title: String,
    pub status: String,
    pub rows: Vec<RowView>,
    pub focused: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    Menu,
    Audio,
    Bluetooth,
    /// Keyed by object path: BlueZ can change an LE device's address.
    Device {
        path: String,
    },
    Add,
    Mirroring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Busy {
    op: BtOp,
    /// Object path, which (unlike the address) never changes for a device.
    path: String,
    since: Instant,
}

#[derive(Debug, Clone)]
struct Pending {
    /// Matched against the Bluetooth output's address.
    address: String,
    path: String,
    deadline: Instant,
}

#[derive(Debug, Clone)]
struct Frame {
    page: Page,
    focus: usize,
    /// The focused row's key, to find it again after the rows change.
    key: Option<String>,
}

impl Frame {
    fn new(page: Page) -> Self {
        Frame {
            page,
            focus: 0,
            key: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Settings {
    stack: Vec<Frame>,
    busy: Option<Busy>,
    pending: Option<Pending>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Whether data should keep being refreshed (open, or waiting to switch).
    pub fn wants_data(&self) -> bool {
        self.is_open() || self.pending.is_some()
    }

    pub fn open(&mut self) -> Vec<Effect> {
        if self.is_open() {
            return vec![];
        }
        self.stack = vec![Frame::new(Page::Menu)];
        vec![Effect::SetBtActive(true)]
    }

    pub fn nav(&mut self, nav: Nav, snap: &Snapshot, now: Instant) -> Vec<Effect> {
        if !self.is_open() {
            return vec![];
        }
        match nav {
            Nav::Home => self.close(),
            Nav::Back => self.back(),
            Nav::Up | Nav::Down => {
                let rows = self.rows(snap);
                if let Some(frame) = self.stack.last_mut() {
                    frame.focus = match nav {
                        Nav::Up => frame.focus.saturating_sub(1),
                        _ => (frame.focus + 1).min(rows.len().saturating_sub(1)),
                    };
                    frame.key = rows.get(frame.focus).map(|r| r.key.clone());
                }
                vec![]
            }
            Nav::Select => self.select(snap, now),
        }
    }

    /// Pointer hover: focus a row directly.
    pub fn set_focus(&mut self, row: usize, snap: &Snapshot) {
        let rows = self.rows(snap);
        if let Some(frame) = self.stack.last_mut() {
            frame.focus = row.min(rows.len().saturating_sub(1));
            frame.key = rows.get(frame.focus).map(|r| r.key.clone());
        }
    }

    pub fn on_data(&mut self, snap: &Snapshot, now: Instant) -> Vec<Effect> {
        let mut effects = vec![];
        if self
            .busy
            .as_ref()
            .is_some_and(|b| now >= b.since + BUSY_TIMEOUT)
        {
            self.busy = None;
        }
        if let Some(pending) = &self.pending {
            if let Some(id) = bt_output_id(snap, &pending.address) {
                effects.push(Effect::SetOutput {
                    id,
                    fallback: tv_output_id(snap),
                });
                self.pending = None;
            } else if now >= pending.deadline {
                effects.push(Effect::Toast(format!(
                    "Couldn't connect {}",
                    device_name(snap, &pending.address)
                )));
                self.pending = None;
            }
        }
        // A device page whose device is gone (forgotten) goes back.
        if let Some(Frame {
            page: Page::Device { path },
            ..
        }) = self.stack.last()
            && !snap.devices.iter().any(|d| d.paired && &d.path == path)
        {
            self.stack.pop();
        }
        self.refocus(snap);
        effects
    }

    /// `path`: the device's object path.
    pub fn op_finished(
        &mut self,
        op: BtOp,
        path: &str,
        result: Result<(), String>,
        snap: &Snapshot,
    ) -> Vec<Effect> {
        let ours = self
            .busy
            .as_ref()
            .is_some_and(|b| b.op == op && b.path == path);
        if ours {
            self.busy = None;
        }
        let name = device_name_by_path(snap, path);
        match result {
            Err(_) => {
                if op == BtOp::Connect && self.pending.as_ref().is_some_and(|p| p.path == path) {
                    self.pending = None;
                }
                let verb = match op {
                    BtOp::Pair => "pair",
                    BtOp::Connect => "connect",
                    BtOp::Disconnect => "disconnect",
                    BtOp::Forget => "forget",
                };
                vec![Effect::Toast(format!("Couldn't {verb} {name}"))]
            }
            Ok(()) => {
                if ours && op == BtOp::Pair && self.top() == Some(&Page::Add) {
                    self.stack.pop();
                    return vec![Effect::StopScan];
                }
                vec![]
            }
        }
    }

    pub fn view(&self, snap: &Snapshot) -> View {
        let rows = self.rows(snap);
        let focused = self
            .stack
            .last()
            .map_or(0, |f| f.focus.min(rows.len().saturating_sub(1)));
        let (title, status) = match self.top() {
            None | Some(Page::Menu) => ("Settings".to_string(), String::new()),
            Some(Page::Audio) => ("Audio output".to_string(), String::new()),
            Some(Page::Mirroring) => ("Mirroring".to_string(), String::new()),
            Some(Page::Bluetooth) => (
                "Bluetooth".to_string(),
                if snap.bt_available {
                    String::new()
                } else {
                    "Bluetooth isn't available".to_string()
                },
            ),
            Some(Page::Device { path }) => (device_name_by_path(snap, path), String::new()),
            Some(Page::Add) => {
                let status = match (&snap.passkey, &self.busy) {
                    (Some(code), Some(b)) => {
                        format!(
                            "Type {code} on {}, then Enter",
                            device_name_by_path(snap, &b.path)
                        )
                    }
                    (_, Some(b)) if b.op == BtOp::Pair => "Pairing…".to_string(),
                    _ if snap.scanning => "Searching…".to_string(),
                    _ => String::new(),
                };
                ("Add device".to_string(), status)
            }
        };
        View {
            title,
            status,
            rows,
            focused,
        }
    }

    fn top(&self) -> Option<&Page> {
        self.stack.last().map(|f| &f.page)
    }

    fn focus(&self) -> usize {
        self.stack.last().map_or(0, |f| f.focus)
    }

    fn close(&mut self) -> Vec<Effect> {
        let mut effects = vec![];
        if self.top() == Some(&Page::Add) {
            effects.push(Effect::StopScan);
        }
        self.stack.clear();
        effects.extend([Effect::SetBtActive(false), Effect::Close]);
        effects
    }

    fn back(&mut self) -> Vec<Effect> {
        if self.stack.len() <= 1 {
            return self.close();
        }
        let left = self.stack.pop().map(|f| f.page);
        if left == Some(Page::Add) {
            vec![Effect::StopScan]
        } else {
            vec![]
        }
    }

    fn push(&mut self, page: Page) {
        self.stack.push(Frame::new(page));
    }

    /// After the rows changed: once the user has moved, keep focus on the
    /// same row (by key) if it's still there; otherwise clamp to the list.
    /// Before any move, focus stays put (the top), however the list fills.
    fn refocus(&mut self, snap: &Snapshot) {
        let rows = self.rows(snap);
        if let Some(frame) = self.stack.last_mut() {
            let found = frame
                .key
                .as_ref()
                .and_then(|key| rows.iter().position(|r| &r.key == key));
            frame.focus = found.unwrap_or(frame.focus.min(rows.len().saturating_sub(1)));
            if frame.key.is_some() {
                frame.key = rows.get(frame.focus).map(|r| r.key.clone());
            }
        }
    }

    fn select(&mut self, snap: &Snapshot, now: Instant) -> Vec<Effect> {
        // One Bluetooth operation at a time; moving between pages is fine.
        let op_running = self.busy.is_some();
        let focus = self.focus();
        match self.top().cloned() {
            None => vec![],
            Some(Page::Menu) => match focus {
                0 => {
                    self.push(Page::Audio);
                    vec![]
                }
                2 => {
                    self.push(Page::Mirroring);
                    vec![]
                }
                _ => {
                    self.push(Page::Bluetooth);
                    if snap.bt_available {
                        vec![Effect::PowerOn]
                    } else {
                        vec![]
                    }
                }
            },
            Some(Page::Audio) => {
                if focus == 0 {
                    return match tv_output_id(snap) {
                        Some(id) => {
                            self.pending = None; // the latest choice wins
                            vec![Effect::SetOutput { id, fallback: None }]
                        }
                        None => vec![Effect::Toast("TV audio isn't available".into())],
                    };
                }
                let Some(device) = audio_devices(snap).get(focus - 1).cloned() else {
                    return vec![];
                };
                if let Some(id) = bt_output_id(snap, &device.address) {
                    self.pending = None;
                    return vec![Effect::SetOutput {
                        id,
                        fallback: tv_output_id(snap),
                    }];
                }
                if !device.connected && op_running {
                    return vec![];
                }
                self.pending = Some(Pending {
                    address: device.address.clone(),
                    path: device.path.clone(),
                    deadline: now + CONNECT_SWITCH_TIMEOUT,
                });
                if device.connected {
                    vec![]
                } else {
                    self.busy = Some(Busy {
                        op: BtOp::Connect,
                        path: device.path.clone(),
                        since: now,
                    });
                    vec![Effect::Connect(device.path)]
                }
            }
            Some(Page::Bluetooth) => {
                let paired = paired_devices(snap);
                if !snap.bt_available {
                    return vec![];
                }
                if focus < paired.len() {
                    self.push(Page::Device {
                        path: paired[focus].path.clone(),
                    });
                    vec![]
                } else {
                    self.push(Page::Add);
                    vec![Effect::StartScan]
                }
            }
            Some(Page::Device { path }) => {
                let Some(device) = snap.devices.iter().find(|d| d.path == path).cloned() else {
                    return vec![];
                };
                match focus {
                    0 if op_running => vec![],
                    0 if device.connected => {
                        self.busy = Some(Busy {
                            op: BtOp::Disconnect,
                            path,
                            since: now,
                        });
                        vec![Effect::Disconnect(device.path)]
                    }
                    0 => {
                        self.busy = Some(Busy {
                            op: BtOp::Connect,
                            path,
                            since: now,
                        });
                        vec![Effect::Connect(device.path)]
                    }
                    _ => {
                        self.stack.pop();
                        vec![Effect::Forget(device.path)]
                    }
                }
            }
            Some(Page::Mirroring) => {
                let mode = match focus {
                    0 => MirrorMode::LowLag,
                    _ => MirrorMode::Smooth,
                };
                vec![Effect::SetMirroringMode(mode)]
            }
            Some(Page::Add) => {
                let Some(device) = new_devices(snap).get(focus).cloned() else {
                    return vec![];
                };
                if op_running {
                    return vec![];
                }
                self.busy = Some(Busy {
                    op: BtOp::Pair,
                    path: device.path.clone(),
                    since: now,
                });
                vec![Effect::Pair(device.path)]
            }
        }
    }

    fn rows(&self, snap: &Snapshot) -> Vec<RowView> {
        let row = |key: &str, icon, label: &str, detail: String, checked| RowView {
            key: key.to_string(),
            icon,
            label: label.to_string(),
            detail,
            checked,
        };
        match self.top() {
            None => vec![],
            Some(Page::Menu) => {
                let bt_detail = if !snap.bt_available {
                    "Not available".to_string()
                } else if !snap.powered {
                    "Off".to_string()
                } else {
                    match paired_devices(snap).len() {
                        0 => "No devices".to_string(),
                        1 => "1 device".to_string(),
                        n => format!("{n} devices"),
                    }
                };
                vec![
                    row(
                        "audio",
                        Icon::AudioOutput,
                        "Audio output",
                        self.chosen_output_name(snap),
                        false,
                    ),
                    row("bluetooth", Icon::Bluetooth, "Bluetooth", bt_detail, false),
                    row(
                        "mirroring",
                        Icon::Mirroring,
                        "Mirroring",
                        snap.mirroring_mode.label().to_string(),
                        false,
                    ),
                ]
            }
            Some(Page::Audio) => {
                let chosen = chosen_bt_address(snap);
                let mut rows = vec![row("tv", Icon::Tv, "TV", String::new(), chosen.is_none())];
                rows.extend(audio_devices(snap).into_iter().map(|d| {
                    let checked = chosen.as_deref() == Some(d.address.as_str());
                    row(
                        &d.path,
                        Icon::Speaker,
                        &d.name,
                        self.status_text(&d),
                        checked,
                    )
                }));
                rows
            }
            Some(Page::Mirroring) => [
                ("low-lag", Icon::LowLag, MirrorMode::LowLag),
                ("smooth", Icon::Smooth, MirrorMode::Smooth),
            ]
            .into_iter()
            .map(|(key, icon, mode)| {
                row(
                    key,
                    icon,
                    mode.label(),
                    String::new(),
                    snap.mirroring_mode == mode,
                )
            })
            .collect(),
            Some(Page::Bluetooth) => {
                if !snap.bt_available {
                    return vec![];
                }
                let mut rows: Vec<RowView> = paired_devices(snap)
                    .into_iter()
                    .map(|d| {
                        row(
                            &d.path,
                            icon_for(d.kind),
                            &d.name,
                            self.status_text(&d),
                            false,
                        )
                    })
                    .collect();
                rows.push(row("add", Icon::Add, "Add device", String::new(), false));
                rows
            }
            Some(Page::Device { path }) => {
                let Some(device) = snap.devices.iter().find(|d| &d.path == path) else {
                    return vec![];
                };
                let first = if device.connected {
                    row("link", Icon::Disconnect, "Disconnect", String::new(), false)
                } else {
                    row("link", Icon::Connect, "Connect", String::new(), false)
                };
                vec![
                    first,
                    row("forget", Icon::Forget, "Forget", String::new(), false),
                ]
            }
            Some(Page::Add) => new_devices(snap)
                .into_iter()
                .map(|d| {
                    let detail = match &self.busy {
                        Some(b) if b.op == BtOp::Pair && b.path == d.path => "Pairing…".to_string(),
                        _ => String::new(),
                    };
                    row(&d.path, icon_for(d.kind), &d.name, detail, false)
                })
                .collect(),
        }
    }

    fn status_text(&self, d: &BtDevice) -> String {
        let busy_op = self
            .busy
            .as_ref()
            .filter(|b| b.path == d.path)
            .map(|b| b.op);
        let pending = self.pending.as_ref().is_some_and(|p| p.path == d.path);
        match busy_op {
            Some(BtOp::Connect) => "Connecting…".to_string(),
            Some(BtOp::Disconnect) => "Disconnecting…".to_string(),
            _ if pending => "Connecting…".to_string(),
            _ if d.connected => "Connected".to_string(),
            _ => "Not connected".to_string(),
        }
    }

    fn chosen_output_name(&self, snap: &Snapshot) -> String {
        match chosen_bt_address(snap) {
            Some(address) => device_name(snap, &address),
            None => "TV".to_string(),
        }
    }
}

/// The chosen speaker's address, if the stored choice is a Bluetooth output.
fn chosen_bt_address(snap: &Snapshot) -> Option<String> {
    let configured = snap.audio.as_ref()?.configured.as_deref()?;
    bt_address_from_node_name(configured)
}

fn tv_output_id(snap: &Snapshot) -> Option<u32> {
    snap.audio
        .as_ref()?
        .outputs
        .iter()
        .find(|o| o.kind == OutputKind::Tv)
        .map(|o| o.id)
}

fn bt_output_id(snap: &Snapshot, address: &str) -> Option<u32> {
    snap.audio
        .as_ref()?
        .outputs
        .iter()
        .find(|o| o.kind == OutputKind::Bluetooth && o.bt_address.as_deref() == Some(address))
        .map(|o| o.id)
}

fn device_name_by_path(snap: &Snapshot, path: &str) -> String {
    snap.devices.iter().find(|d| d.path == path).map_or_else(
        || crate::bluetooth::address_from_path(path),
        |d| d.name.clone(),
    )
}

fn device_name(snap: &Snapshot, address: &str) -> String {
    snap.devices
        .iter()
        .find(|d| d.address == address)
        .map_or_else(|| address.to_string(), |d| d.name.clone())
}

fn sorted(mut devices: Vec<BtDevice>) -> Vec<BtDevice> {
    devices.sort_by_key(|d| d.name.to_lowercase());
    devices
}

fn paired_devices(snap: &Snapshot) -> Vec<BtDevice> {
    sorted(snap.devices.iter().filter(|d| d.paired).cloned().collect())
}

fn audio_devices(snap: &Snapshot) -> Vec<BtDevice> {
    sorted(
        snap.devices
            .iter()
            .filter(|d| d.paired && d.kind == DeviceKind::Audio)
            .cloned()
            .collect(),
    )
}

/// Unpaired, named devices nearby. Known types (speakers, controllers,
/// keyboards, mice) come first: neighbours' sensors and appliances also
/// advertise names, and would otherwise bury what you're pairing.
fn new_devices(snap: &Snapshot) -> Vec<BtDevice> {
    let mut devices = sorted(
        snap.devices
            .iter()
            .filter(|d| !d.paired && d.has_name)
            .cloned()
            .collect(),
    );
    devices.sort_by_key(|d| d.kind == DeviceKind::Other); // stable: keeps name order
    devices
}

fn icon_for(kind: DeviceKind) -> Icon {
    match kind {
        DeviceKind::Audio => Icon::Speaker,
        DeviceKind::Gamepad => Icon::Gamepad,
        DeviceKind::Keyboard => Icon::Keyboard,
        DeviceKind::Mouse => Icon::Mouse,
        DeviceKind::Other => Icon::Device,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Output;

    const SPK: &str = "00:11:22:AA:BB:CC";
    const PAD: &str = "11:22:33:44:55:66";
    const NEW: &str = "77:88:99:AA:BB:CC";

    fn dev(address: &str, name: &str, kind: DeviceKind, paired: bool, connected: bool) -> BtDevice {
        BtDevice {
            path: format!("/org/bluez/hci0/dev_{}", address.replace(':', "_")),
            address: address.into(),
            name: name.into(),
            has_name: true,
            kind,
            paired,
            connected,
        }
    }

    fn path(address: &str) -> String {
        format!("/org/bluez/hci0/dev_{}", address.replace(':', "_"))
    }

    fn tv_output() -> Output {
        Output {
            id: 52,
            name: "alsa_output.pci-0000_00_1f.3.hdmi-stereo".into(),
            description: "HDMI".into(),
            kind: OutputKind::Tv,
            bt_address: None,
        }
    }

    fn spk_output() -> Output {
        Output {
            id: 77,
            name: "bluez_output.00_11_22_AA_BB_CC.1".into(),
            description: "JBL".into(),
            kind: OutputKind::Bluetooth,
            bt_address: Some(SPK.into()),
        }
    }

    /// Paired speaker (disconnected) and gamepad (connected), plus one
    /// unpaired device nearby; audio has only the TV.
    fn snap() -> Snapshot {
        Snapshot {
            bt_available: true,
            powered: true,
            scanning: false,
            devices: vec![
                dev(SPK, "JBL Flip", DeviceKind::Audio, true, false),
                dev(PAD, "Xbox Controller", DeviceKind::Gamepad, true, true),
                dev(NEW, "Sony Speaker", DeviceKind::Audio, false, false),
            ],
            audio: Some(AudioState {
                outputs: vec![tv_output()],
                configured: None,
            }),
            passkey: None,
            mirroring_mode: MirrorMode::LowLag,
        }
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn open(s: &Snapshot) -> Settings {
        let mut st = Settings::new();
        st.open();
        let _ = s;
        st
    }

    fn labels(v: &View) -> Vec<&str> {
        v.rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn opening_activates_bluetooth_polling() {
        let mut st = Settings::new();
        assert!(!st.is_open());
        assert_eq!(st.open(), vec![Effect::SetBtActive(true)]);
        assert!(st.is_open());
        let v = st.view(&snap());
        assert_eq!(v.title, "Settings");
        assert_eq!(labels(&v), ["Audio output", "Bluetooth", "Mirroring"]);
    }

    #[test]
    fn open_while_open_is_a_no_op() {
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        assert_eq!(st.open(), vec![]);
        assert_eq!(st.view(&s).title, "Add device", "the page stack was reset");
    }

    #[test]
    fn menu_details_show_current_state() {
        let st = open(&snap());
        let v = st.view(&snap());
        assert_eq!(v.rows[0].detail, "TV");
        assert_eq!(v.rows[1].detail, "2 devices");
        let mut off = snap();
        off.powered = false;
        assert_eq!(st.view(&off).rows[1].detail, "Off");
        let mut none = snap();
        none.bt_available = false;
        assert_eq!(st.view(&none).rows[1].detail, "Not available");
        let mut empty = snap();
        empty.devices.clear();
        assert_eq!(st.view(&empty).rows[1].detail, "No devices");
    }

    #[test]
    fn menu_shows_chosen_speaker_name() {
        let st = open(&snap());
        let mut s = snap();
        s.audio = Some(AudioState {
            outputs: vec![tv_output()],
            configured: Some("bluez_output.00_11_22_AA_BB_CC.1".into()),
        });
        assert_eq!(st.view(&s).rows[0].detail, "JBL Flip");
    }

    #[test]
    fn back_walks_up_and_closes_from_the_menu() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now()); // Audio output
        assert_eq!(st.view(&s).title, "Audio output");
        assert_eq!(st.nav(Nav::Back, &s, now()), vec![]);
        assert_eq!(st.view(&s).title, "Settings");
        assert_eq!(
            st.nav(Nav::Back, &s, now()),
            vec![Effect::SetBtActive(false), Effect::Close]
        );
        assert!(!st.is_open());
    }

    #[test]
    fn focus_moves_and_is_clamped() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Up, &s, now());
        assert_eq!(st.view(&s).focused, 0);
        st.nav(Nav::Down, &s, now());
        st.nav(Nav::Down, &s, now());
        st.nav(Nav::Down, &s, now());
        assert_eq!(st.view(&s).focused, 2, "clamped to the last menu row");
    }

    #[test]
    fn back_returns_focus_to_the_parent_row() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Down, &s, now());
        st.nav(Nav::Select, &s, now()); // Bluetooth
        st.nav(Nav::Back, &s, now());
        assert_eq!(st.view(&s).focused, 1);
    }

    #[test]
    fn audio_page_lists_tv_and_paired_audio_devices() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now());
        let v = st.view(&s);
        assert_eq!(labels(&v), ["TV", "JBL Flip"]);
        assert!(v.rows[0].checked, "no stored choice means TV");
        assert!(!v.rows[1].checked);
        assert_eq!(v.rows[1].detail, "Not connected");
        assert_eq!(v.rows[0].icon, Icon::Tv);
        assert_eq!(v.rows[1].icon, Icon::Speaker);
    }

    #[test]
    fn audio_page_checks_the_chosen_speaker() {
        let mut s = snap();
        s.audio = Some(AudioState {
            outputs: vec![tv_output()],
            configured: Some("bluez_output.00_11_22_AA_BB_CC.1".into()),
        });
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now());
        let v = st.view(&s);
        assert!(!v.rows[0].checked);
        assert!(v.rows[1].checked);
    }

    #[test]
    fn choosing_tv_sets_the_hdmi_output() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::SetOutput {
                id: 52,
                fallback: None
            }]
        );
    }

    #[test]
    fn choosing_tv_without_an_hdmi_output_explains() {
        let mut s = snap();
        s.audio = Some(AudioState::default());
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now());
        let effects = st.nav(Nav::Select, &s, now());
        assert!(
            matches!(effects.as_slice(), [Effect::Toast(_)]),
            "{effects:?}"
        );
    }

    #[test]
    fn choosing_a_connected_speaker_switches_now() {
        let mut s = snap();
        s.devices[0].connected = true;
        s.audio = Some(AudioState {
            outputs: vec![tv_output(), spk_output()],
            configured: None,
        });
        let mut st = open(&s);
        st.nav(Nav::Select, &s, now());
        st.nav(Nav::Down, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::SetOutput {
                id: 77,
                fallback: Some(52)
            }]
        );
    }

    #[test]
    fn choosing_a_disconnected_speaker_connects_then_switches() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        assert_eq!(st.nav(Nav::Select, &s, t), vec![Effect::Connect(path(SPK))]);
        assert_eq!(st.view(&s).rows[1].detail, "Connecting…");
        // Connected, but no output yet: keep waiting.
        assert_eq!(st.on_data(&s, t + Duration::from_secs(2)), vec![]);
        let mut later = snap();
        later.devices[0].connected = true;
        later.audio = Some(AudioState {
            outputs: vec![tv_output(), spk_output()],
            configured: None,
        });
        assert_eq!(
            st.on_data(&later, t + Duration::from_secs(3)),
            vec![Effect::SetOutput {
                id: 77,
                fallback: Some(52)
            }]
        );
        assert_eq!(st.on_data(&later, t + Duration::from_secs(4)), vec![]);
    }

    #[test]
    fn pending_switch_times_out() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t);
        assert_eq!(st.on_data(&s, t + Duration::from_secs(14)), vec![]);
        assert_eq!(
            st.on_data(&s, t + CONNECT_SWITCH_TIMEOUT),
            vec![Effect::Toast("Couldn't connect JBL Flip".into())]
        );
        assert!(!st.wants_data() || st.is_open());
    }

    #[test]
    fn failed_connect_cancels_the_pending_switch() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t);
        let effects = st.op_finished(BtOp::Connect, &path(SPK), Err("host down".into()), &s);
        assert_eq!(
            effects,
            vec![Effect::Toast("Couldn't connect JBL Flip".into())]
        );
        assert_eq!(st.on_data(&s, t + Duration::from_secs(20)), vec![]);
    }

    #[test]
    fn pending_switch_survives_closing_settings() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Home, &s, t);
        assert!(!st.is_open());
        assert!(st.wants_data());
        let mut later = snap();
        later.audio = Some(AudioState {
            outputs: vec![tv_output(), spk_output()],
            configured: None,
        });
        assert_eq!(
            st.on_data(&later, t + Duration::from_secs(5)),
            vec![Effect::SetOutput {
                id: 77,
                fallback: Some(52)
            }]
        );
        assert!(!st.wants_data());
    }

    #[test]
    fn selects_are_ignored_while_an_operation_runs() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t);
        assert_eq!(st.nav(Nav::Select, &s, t), vec![]);
    }

    #[test]
    fn bluetooth_page_lists_paired_devices_and_add() {
        let s = snap();
        let mut st = open(&s);
        st.nav(Nav::Down, &s, now());
        assert_eq!(st.nav(Nav::Select, &s, now()), vec![Effect::PowerOn]);
        let v = st.view(&s);
        assert_eq!(v.title, "Bluetooth");
        assert_eq!(labels(&v), ["JBL Flip", "Xbox Controller", "Add device"]);
        assert_eq!(v.rows[1].detail, "Connected");
        assert_eq!(v.rows[1].icon, Icon::Gamepad);
        assert_eq!(v.rows[2].icon, Icon::Add);
    }

    #[test]
    fn bluetooth_page_without_adapter_explains() {
        let mut s = snap();
        s.bt_available = false;
        let mut st = open(&s);
        st.nav(Nav::Down, &s, now());
        assert_eq!(st.nav(Nav::Select, &s, now()), vec![]);
        let v = st.view(&s);
        assert!(v.rows.is_empty());
        assert_eq!(v.status, "Bluetooth isn't available");
    }

    fn to_device_page(st: &mut Settings, s: &Snapshot, row: usize) {
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Select, s, now());
        for _ in 0..row {
            st.nav(Nav::Down, s, now());
        }
        st.nav(Nav::Select, s, now());
    }

    #[test]
    fn device_page_offers_connect_or_disconnect_and_forget() {
        let s = snap();
        let mut st = open(&s);
        to_device_page(&mut st, &s, 1); // Xbox Controller, connected
        let v = st.view(&s);
        assert_eq!(v.title, "Xbox Controller");
        assert_eq!(labels(&v), ["Disconnect", "Forget"]);
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::Disconnect(path(PAD))]
        );

        let mut st = open(&s);
        to_device_page(&mut st, &s, 0); // JBL, disconnected
        assert_eq!(labels(&st.view(&s)), ["Connect", "Forget"]);
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::Connect(path(SPK))]
        );
    }

    #[test]
    fn forget_removes_and_goes_back() {
        let s = snap();
        let mut st = open(&s);
        to_device_page(&mut st, &s, 0);
        st.nav(Nav::Down, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::Forget(path(SPK))]
        );
        assert_eq!(st.view(&s).title, "Bluetooth");
    }

    #[test]
    fn device_page_closes_when_the_device_disappears() {
        let s = snap();
        let mut st = open(&s);
        to_device_page(&mut st, &s, 0);
        let mut gone = snap();
        gone.devices.remove(0);
        st.on_data(&gone, now());
        assert_eq!(st.view(&gone).title, "Bluetooth");
    }

    fn to_add_page(st: &mut Settings, s: &Snapshot) -> Vec<Effect> {
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Select, s, now());
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Select, s, now())
    }

    #[test]
    fn add_page_scans_while_open() {
        let s = snap();
        let mut st = open(&s);
        assert_eq!(to_add_page(&mut st, &s), vec![Effect::StartScan]);
        let mut scanning = snap();
        scanning.scanning = true;
        let v = st.view(&scanning);
        assert_eq!(v.title, "Add device");
        assert_eq!(v.status, "Searching…");
        assert_eq!(labels(&v), ["Sony Speaker"]);
        assert_eq!(st.nav(Nav::Back, &scanning, now()), vec![Effect::StopScan]);
        assert_eq!(st.view(&s).title, "Bluetooth");
    }

    #[test]
    fn add_page_lists_known_device_types_first() {
        let mut s = snap();
        s.devices.push(dev(
            "01:01:01:01:01:01",
            "Air purifier",
            DeviceKind::Other,
            false,
            false,
        ));
        s.devices.push(dev(
            "02:02:02:02:02:02",
            "Xbox Wireless",
            DeviceKind::Gamepad,
            false,
            false,
        ));
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        assert_eq!(
            labels(&st.view(&s)),
            ["Sony Speaker", "Xbox Wireless", "Air purifier"]
        );
    }

    #[test]
    fn add_page_hides_nameless_devices() {
        let mut s = snap();
        s.devices[2].has_name = false;
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        assert!(st.view(&s).rows.is_empty());
    }

    #[test]
    fn close_stops_scan() {
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        assert_eq!(
            st.nav(Nav::Home, &s, now()),
            vec![Effect::StopScan, Effect::SetBtActive(false), Effect::Close]
        );
    }

    #[test]
    fn pairing_success_returns_to_the_device_list() {
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::Pair(path(NEW))]
        );
        assert_eq!(st.view(&s).status, "Pairing…");
        assert_eq!(st.view(&s).rows[0].detail, "Pairing…");
        let effects = st.op_finished(BtOp::Pair, &path(NEW), Ok(()), &s);
        assert_eq!(effects, vec![Effect::StopScan]);
        assert_eq!(st.view(&s).title, "Bluetooth");
    }

    #[test]
    fn pairing_failure_shows_a_toast_and_stays() {
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Select, &s, now());
        let effects = st.op_finished(
            BtOp::Pair,
            &path(NEW),
            Err("AuthenticationFailed".into()),
            &s,
        );
        assert_eq!(
            effects,
            vec![Effect::Toast("Couldn't pair Sony Speaker".into())]
        );
        assert_eq!(st.view(&s).title, "Add device");
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::Pair(path(NEW))]
        );
    }

    #[test]
    fn passkey_is_shown_while_pairing() {
        let mut s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Select, &s, now());
        s.passkey = Some("123456".into());
        assert_eq!(
            st.view(&s).status,
            "Type 123456 on Sony Speaker, then Enter"
        );
    }

    #[test]
    fn op_result_for_stale_page_is_ignored() {
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Select, &s, now());
        st.nav(Nav::Home, &s, now());
        // Pairing finishes after Settings was closed: no navigation, no scan stop.
        assert_eq!(st.op_finished(BtOp::Pair, &path(NEW), Ok(()), &s), vec![]);
        assert!(!st.is_open());
    }

    #[test]
    fn focus_clamped_when_rows_shrink() {
        let mut s = snap();
        s.devices.push(dev(
            "AA:AA:AA:AA:AA:AA",
            "Another",
            DeviceKind::Audio,
            false,
            false,
        ));
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Down, &s, now());
        assert_eq!(st.view(&s).focused, 1);
        let mut fewer = snap();
        fewer.devices.truncate(2); // both unpaired devices gone
        st.on_data(&fewer, now());
        let v = st.view(&fewer);
        assert!(v.rows.is_empty());
        assert_eq!(v.focused, 0);
        assert_eq!(st.nav(Nav::Select, &fewer, now()), vec![]);
    }

    #[test]
    fn pointer_focus_is_clamped() {
        let s = snap();
        let mut st = open(&s);
        st.set_focus(1, &s);
        assert_eq!(st.view(&s).focused, 1);
        st.set_focus(9, &s);
        assert_eq!(st.view(&s).focused, 2);
    }

    #[test]
    fn op_finished_matches_by_path_even_if_the_address_changed() {
        // BlueZ can replace an LE device's random address with its identity
        // address during pairing; the object path stays the same.
        let s = snap();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Select, &s, now());
        let mut resolved = snap();
        resolved.devices[2].address = "C0:FF:EE:C0:FF:EE".into();
        let effects = st.op_finished(BtOp::Pair, &path(NEW), Ok(()), &resolved);
        assert_eq!(effects, vec![Effect::StopScan]);
        assert_eq!(st.view(&resolved).title, "Bluetooth");
        // Nothing is left marked busy: a new operation can start.
        st.nav(Nav::Up, &resolved, now());
        st.nav(Nav::Up, &resolved, now());
        st.nav(Nav::Select, &resolved, now()); // JBL Flip's page
        assert_eq!(
            st.nav(Nav::Select, &resolved, now()),
            vec![Effect::Connect(path(SPK))]
        );
    }

    #[test]
    fn navigation_works_while_an_op_runs() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t); // Audio
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t); // connect the speaker
        st.nav(Nav::Back, &s, t);
        st.nav(Nav::Down, &s, t);
        assert_eq!(st.nav(Nav::Select, &s, t), vec![Effect::PowerOn]);
        assert_eq!(st.view(&s).title, "Bluetooth");
        st.nav(Nav::Select, &s, t); // device page opens too
        assert_eq!(st.view(&s).title, "JBL Flip");
    }

    #[test]
    fn a_second_op_waits_for_the_first() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        to_device_page(&mut st, &s, 0);
        assert_eq!(st.nav(Nav::Select, &s, t), vec![Effect::Connect(path(SPK))]);
        st.nav(Nav::Back, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t); // Xbox Controller page
        assert_eq!(
            st.nav(Nav::Select, &s, t),
            vec![],
            "Disconnect while Connect runs"
        );
    }

    #[test]
    fn stuck_operation_expires() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Select, &s, t);
        st.on_data(&s, t + BUSY_TIMEOUT);
        assert_eq!(
            st.nav(Nav::Select, &s, t + BUSY_TIMEOUT),
            vec![Effect::Pair(path(NEW))]
        );
    }

    #[test]
    fn choosing_tv_cancels_a_pending_switch() {
        let s = snap();
        let t = now();
        let mut st = open(&s);
        st.nav(Nav::Select, &s, t);
        st.nav(Nav::Down, &s, t);
        st.nav(Nav::Select, &s, t); // connect, then switch to the speaker…
        st.nav(Nav::Up, &s, t);
        assert_eq!(
            st.nav(Nav::Select, &s, t),
            vec![Effect::SetOutput {
                id: 52,
                fallback: None
            }]
        ); // …but TV now
        let mut later = snap();
        later.devices[0].connected = true;
        later.audio = Some(AudioState {
            outputs: vec![tv_output(), spk_output()],
            configured: None,
        });
        assert_eq!(st.on_data(&later, t + Duration::from_secs(3)), vec![]);
    }

    #[test]
    fn focus_follows_the_device_when_the_list_reorders() {
        let mut s = snap();
        s.devices.push(dev(
            "0A:0A:0A:0A:0A:0A",
            "Zebra Speaker",
            DeviceKind::Audio,
            false,
            false,
        ));
        let t = now();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        st.nav(Nav::Down, &s, t); // Sony, Zebra → Zebra
        assert_eq!(st.view(&s).rows[st.view(&s).focused].label, "Zebra Speaker");
        // A new speaker shows up and sorts above both.
        s.devices.push(dev(
            "0B:0B:0B:0B:0B:0B",
            "Bose",
            DeviceKind::Audio,
            false,
            false,
        ));
        st.on_data(&s, t);
        let v = st.view(&s);
        assert_eq!(v.rows[v.focused].label, "Zebra Speaker");
        assert_eq!(
            st.nav(Nav::Select, &s, t),
            vec![Effect::Pair(path("0A:0A:0A:0A:0A:0A"))]
        );
    }

    #[test]
    fn initial_focus_stays_at_the_top_while_the_list_fills() {
        let mut s = snap();
        s.devices.truncate(2); // nothing unpaired yet
        let t = now();
        let mut st = open(&s);
        to_add_page(&mut st, &s);
        s.devices.push(dev(
            "0C:0C:0C:0C:0C:0C",
            "Sony",
            DeviceKind::Audio,
            false,
            false,
        ));
        st.on_data(&s, t);
        s.devices.push(dev(
            "0B:0B:0B:0B:0B:0B",
            "Bose",
            DeviceKind::Audio,
            false,
            false,
        ));
        st.on_data(&s, t);
        let v = st.view(&s);
        assert_eq!(v.focused, 0);
        assert_eq!(v.rows[0].label, "Bose");
    }

    #[test]
    fn menu_shows_the_mirroring_mode() {
        let mut s = snap();
        let st = open(&s);
        assert_eq!(st.view(&s).rows[2].detail, "Low lag");
        assert_eq!(st.view(&s).rows[2].icon, Icon::Mirroring);
        s.mirroring_mode = MirrorMode::Smooth;
        assert_eq!(st.view(&s).rows[2].detail, "Smooth playback");
    }

    fn to_mirroring_page(st: &mut Settings, s: &Snapshot) {
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Select, s, now());
    }

    #[test]
    fn mirroring_page_checks_the_current_mode() {
        let s = snap();
        let mut st = open(&s);
        to_mirroring_page(&mut st, &s);
        let v = st.view(&s);
        assert_eq!(v.title, "Mirroring");
        assert_eq!(labels(&v), ["Low lag", "Smooth playback"]);
        assert!(v.rows[0].checked && !v.rows[1].checked);
    }

    #[test]
    fn choosing_a_mirroring_mode_sets_it() {
        let s = snap();
        let mut st = open(&s);
        to_mirroring_page(&mut st, &s);
        st.nav(Nav::Down, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::SetMirroringMode(MirrorMode::Smooth)]
        );
        st.nav(Nav::Up, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::SetMirroringMode(MirrorMode::LowLag)]
        );
    }

    #[test]
    fn back_from_mirroring_returns_to_its_menu_row() {
        let s = snap();
        let mut st = open(&s);
        to_mirroring_page(&mut st, &s);
        st.nav(Nav::Back, &s, now());
        let v = st.view(&s);
        assert_eq!(v.title, "Settings");
        assert_eq!(v.focused, 2);
    }

    #[test]
    fn home_from_anywhere_closes() {
        let s = snap();
        let mut st = open(&s);
        to_device_page(&mut st, &s, 0);
        assert_eq!(
            st.nav(Nav::Home, &s, now()),
            vec![Effect::SetBtActive(false), Effect::Close]
        );
        assert!(!st.is_open());
    }
}
