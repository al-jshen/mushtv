//! Connects the Settings state machine to the window, the Bluetooth and
//! Wi-Fi workers and PipeWire. Everything here runs on the UI thread; worker
//! results are posted back to it.
//!
//! With `MUSHTV_DEMO` set (screenshots), no worker starts and PipeWire isn't
//! read: Settings shows canned data and drops what it would send.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use crate::audio;
use crate::bluetooth::{self, BluetoothHandle, BtCmd, BtEvent};
use crate::mirroring::MirrorMode;
use crate::prefs::{self, Prefs};
use crate::settings::{Effect, Icon, Nav, Settings, Snapshot};
use crate::wifi::{self, WifiCmd, WifiEvent, WifiHandle};
use crate::{KeyCapData, MainWindow, SettingsRow};

const AUDIO_POLL: Duration = Duration::from_secs(1);

struct Ctl {
    settings: Settings,
    snap: Snapshot,
    /// None in demo mode.
    bt: Option<BluetoothHandle>,
    /// None in demo mode.
    wifi: Option<WifiHandle>,
    demo: bool,
    poll: Timer,
    audio_in_flight: Arc<AtomicBool>,
    weak: Weak<MainWindow>,
    /// Where the launcher's own settings are saved (None: can't save).
    prefs_path: Option<std::path::PathBuf>,
}

thread_local! {
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ctl) -> R) -> Option<R> {
    CTL.with(|ctl| ctl.borrow_mut().as_mut().map(f))
}

/// Starts the Bluetooth and Wi-Fi workers and hooks up the Settings
/// callbacks.
pub fn init(ui: &MainWindow, prefs_path: Option<std::path::PathBuf>, prefs: Prefs) {
    let weak = ui.as_weak();
    let demo = std::env::var_os("MUSHTV_DEMO").is_some();
    let (bt, wifi, snap) = if demo {
        tracing::info!("demo mode: Settings shows canned data");
        (None, None, crate::demo::snapshot(prefs.mirroring_mode))
    } else {
        let bt = bluetooth::spawn({
            let weak = weak.clone();
            move |event| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with(|ctl| ctl.on_bt(event));
                });
            }
        });
        let wifi = wifi::spawn({
            let weak = weak.clone();
            move |event| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with(|ctl| ctl.on_wifi(event));
                });
            }
        });
        let snap = Snapshot {
            mirroring_mode: prefs.mirroring_mode,
            ..Snapshot::default()
        };
        (Some(bt), Some(wifi), snap)
    };
    CTL.with(|ctl| {
        *ctl.borrow_mut() = Some(Ctl {
            settings: Settings::new(),
            snap,
            bt,
            wifi,
            demo,
            poll: Timer::default(),
            audio_in_flight: Arc::new(AtomicBool::new(false)),
            weak,
            prefs_path,
        })
    });
    ui.on_settings_nav(|code| {
        let nav = match code {
            0 => Nav::Up,
            1 => Nav::Down,
            2 => Nav::Select,
            4 => Nav::Left,
            5 => Nav::Right,
            6 => Nav::Delete,
            _ => Nav::Back,
        };
        nav_to(nav);
    });
    ui.on_settings_text(|text| {
        for c in text.chars() {
            nav_to(Nav::Char(c));
        }
    });
    ui.on_settings_keyboard_click(|key| {
        with(|ctl| {
            let snap = ctl.snap.clone();
            let effects = ctl.settings.keyboard_click(
                usize::try_from(key).unwrap_or(0),
                &snap,
                Instant::now(),
            );
            ctl.apply(effects);
        });
    });
    ui.on_settings_hover(|row| {
        with(|ctl| {
            let snap = ctl.snap.clone();
            ctl.settings
                .set_focus(usize::try_from(row).unwrap_or(0), &snap);
            ctl.render();
        });
    });
    ui.on_settings_click(|row| {
        with(|ctl| {
            let snap = ctl.snap.clone();
            ctl.settings
                .set_focus(usize::try_from(row).unwrap_or(0), &snap);
        });
        nav_to(Nav::Select);
    });
}

/// The mirroring mode chosen in Settings (saved in the prefs file).
pub fn mirroring_mode() -> MirrorMode {
    with(|ctl| ctl.snap.mirroring_mode).unwrap_or_default()
}

pub fn is_open() -> bool {
    with(|ctl| ctl.settings.is_open()).unwrap_or(false)
}

pub fn open() {
    with(|ctl| {
        let effects = ctl.settings.open();
        ctl.apply(effects);
    });
}

pub fn nav_to(nav: Nav) {
    with(|ctl| {
        let snap = ctl.snap.clone();
        let effects = ctl.settings.nav(nav, &snap, Instant::now());
        ctl.apply(effects);
    });
}

impl Ctl {
    fn ui(&self) -> Option<MainWindow> {
        self.weak.upgrade()
    }

    fn on_bt(&mut self, event: BtEvent) {
        let effects = match event {
            BtEvent::State(state) => {
                self.snap.bt_available = state.available;
                self.snap.powered = state.powered;
                self.snap.scanning = state.scanning;
                self.snap.devices = state.devices;
                let snap = self.snap.clone();
                self.settings.on_data(&snap, Instant::now())
            }
            BtEvent::OpFinished { op, path, result } => {
                let snap = self.snap.clone();
                self.settings.op_finished(op, &path, result, &snap)
            }
            BtEvent::Passkey(code) => {
                self.snap.passkey = code;
                vec![]
            }
        };
        self.apply(effects);
    }

    fn on_wifi(&mut self, event: WifiEvent) {
        let effects = match event {
            WifiEvent::State(state) => {
                self.snap.wifi = state;
                let snap = self.snap.clone();
                self.settings.on_data(&snap, Instant::now())
            }
            WifiEvent::OpFinished { op, path, result } => {
                let snap = self.snap.clone();
                self.settings.wifi_finished(op, &path, result, &snap)
            }
        };
        self.apply(effects);
    }

    fn send_bt(&self, cmd: BtCmd) {
        match &self.bt {
            Some(bt) => bt.send(cmd),
            None => tracing::info!("demo: ignoring {cmd:?}"),
        }
    }

    fn send_wifi(&self, cmd: WifiCmd) {
        match &self.wifi {
            Some(wifi) => wifi.send(cmd),
            None => tracing::info!("demo: ignoring {cmd:?}"),
        }
    }

    fn on_audio(&mut self, result: anyhow::Result<audio::AudioState>) {
        match result {
            Ok(state) => self.snap.audio = Some(state),
            Err(e) => {
                tracing::warn!("audio: {e:#}");
                self.snap.audio = None;
            }
        }
        let snap = self.snap.clone();
        let effects = self.settings.on_data(&snap, Instant::now());
        self.apply(effects);
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Close => {}
                Effect::SetBtActive(on) => self.send_bt(BtCmd::SetActive(on)),
                Effect::PowerOn => self.send_bt(BtCmd::PowerOn),
                Effect::StartScan => self.send_bt(BtCmd::StartScan),
                Effect::StopScan => self.send_bt(BtCmd::StopScan),
                Effect::Pair(path) => self.send_bt(BtCmd::Pair(path)),
                Effect::Connect(path) => self.send_bt(BtCmd::Connect(path)),
                Effect::Disconnect(path) => self.send_bt(BtCmd::Disconnect(path)),
                Effect::Forget(path) => self.send_bt(BtCmd::Forget(path)),
                Effect::SetOutput { id, fallback } => match fallback
                    .map_or(Ok(()), audio::set_default)
                    .and_then(|()| audio::set_default(id))
                {
                    Ok(()) => {
                        tracing::info!("audio: output {id} chosen");
                        self.poll_audio_now();
                    }
                    Err(e) => {
                        tracing::warn!("audio: {e:#}");
                        if let Some(ui) = self.ui() {
                            crate::show_toast(&ui, "Couldn't switch audio".into());
                        }
                    }
                },
                Effect::SetMirroringMode(mode) => {
                    self.snap.mirroring_mode = mode;
                    let saved = match &self.prefs_path {
                        Some(path) => prefs::update(path, |p| p.mirroring_mode = mode),
                        None => Err(anyhow::anyhow!("no home directory to save settings in")),
                    };
                    match saved {
                        Ok(()) => tracing::info!("mirroring mode: {}", mode.label()),
                        Err(e) => {
                            tracing::warn!("prefs: {e:#}");
                            if let Some(ui) = self.ui() {
                                crate::show_toast(&ui, "Couldn't save the setting".into());
                            }
                        }
                    }
                }
                Effect::Toast(text) => {
                    if let Some(ui) = self.ui() {
                        crate::show_toast(&ui, text);
                    }
                }
                wifi @ (Effect::SetWifiActive(_)
                | Effect::WifiPowerOn
                | Effect::WifiScanning(_)
                | Effect::WifiConnect { .. }
                | Effect::WifiDisconnect(_)
                | Effect::WifiForget(_)) => {
                    if let Some(cmd) = wifi_command(&wifi) {
                        self.send_wifi(cmd);
                    }
                }
            }
        }
        self.update_polling();
        self.render();
    }

    /// Polls audio once a second while Settings needs it.
    fn update_polling(&mut self) {
        let wanted = self.settings.wants_data();
        if wanted && !self.poll.running() {
            self.poll_audio_now();
            self.poll.start(TimerMode::Repeated, AUDIO_POLL, || {
                with(|ctl| ctl.poll_audio_now());
            });
        } else if !wanted && self.poll.running() {
            self.poll.stop();
        }
    }

    /// Reads PipeWire on a short-lived thread so the UI never waits on it.
    fn poll_audio_now(&mut self) {
        if self.demo || self.audio_in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        let in_flight = Arc::clone(&self.audio_in_flight);
        let weak = self.weak.clone();
        std::thread::spawn(move || {
            let result = audio::read();
            in_flight.store(false, Ordering::SeqCst);
            let _ = weak.upgrade_in_event_loop(move |_| {
                with(|ctl| ctl.on_audio(result));
            });
        });
    }

    fn render(&self) {
        let Some(ui) = self.ui() else { return };
        let open = self.settings.is_open();
        ui.set_settings_open(open);
        if !open {
            return;
        }
        let view = self.settings.view(&self.snap);
        let rows: Vec<SettingsRow> = view
            .rows
            .iter()
            .map(|r| SettingsRow {
                icon: glyph(r.icon).into(),
                label: r.label.as_str().into(),
                detail: r.detail.as_str().into(),
                checked: r.checked,
                locked: r.locked,
            })
            .collect();
        ui.set_settings_title(view.title.into());
        ui.set_settings_status(SharedString::from(view.status));
        ui.set_settings_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        ui.set_settings_focused(i32::try_from(view.focused).unwrap_or(0));
        ui.set_settings_keyboard_open(view.keyboard.is_some());
        if let Some(kb) = view.keyboard {
            let keys: Vec<KeyCapData> = kb
                .keys
                .iter()
                .map(|k| KeyCapData {
                    label: k.cap.label.as_str().into(),
                    icon: k.cap.icon,
                    row: i32::try_from(k.row).unwrap_or(0),
                    x: i32::from(k.x),
                    width: i32::from(k.cap.width),
                    focused: k.focused,
                })
                .collect();
            ui.set_settings_keyboard_title(kb.title.into());
            ui.set_settings_keyboard_text(kb.text.into());
            ui.set_settings_keyboard_keys(ModelRc::from(Rc::new(VecModel::from(keys))));
        }
    }
}

/// The Wi-Fi worker's command for a Wi-Fi effect.
fn wifi_command(effect: &Effect) -> Option<WifiCmd> {
    Some(match effect {
        Effect::SetWifiActive(on) => WifiCmd::SetActive(*on),
        Effect::WifiPowerOn => WifiCmd::PowerOn,
        Effect::WifiScanning(on) => WifiCmd::SetScanning(*on),
        Effect::WifiConnect { path, passphrase } => WifiCmd::Connect {
            path: path.clone(),
            passphrase: passphrase.clone(),
        },
        Effect::WifiDisconnect(path) => WifiCmd::Disconnect(path.clone()),
        Effect::WifiForget(path) => WifiCmd::Forget(path.clone()),
        _ => return None,
    })
}

/// Material Icons codepoints.
pub fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::AudioOutput => "\u{e050}",   // volume_up
        Icon::Bluetooth => "\u{e1a7}",     // bluetooth
        Icon::Tv => "\u{e333}",            // tv
        Icon::Speaker => "\u{e32d}",       // speaker
        Icon::Gamepad => "\u{ea28}",       // sports_esports
        Icon::Keyboard => "\u{e312}",      // keyboard
        Icon::Mouse => "\u{e323}",         // mouse
        Icon::Device => "\u{e337}",        // devices_other
        Icon::Add => "\u{e145}",           // add
        Icon::Connect => "\u{e1aa}",       // bluetooth_searching
        Icon::Disconnect => "\u{e16f}",    // link_off
        Icon::Forget => "\u{e872}",        // delete
        Icon::Mirroring => "\u{e307}",     // cast
        Icon::LowLag => "\u{ea0b}",        // bolt
        Icon::Smooth => "\u{e02c}",        // movie
        Icon::Wifi => "\u{e63e}",          // wifi
        Icon::WifiSignal(1) => "\u{ebe4}", // network_wifi_1_bar
        Icon::WifiSignal(2) => "\u{ebd6}", // network_wifi_2_bar
        Icon::WifiSignal(3) => "\u{ebe1}", // network_wifi_3_bar
        Icon::WifiSignal(_) => "\u{e1d8}", // signal_wifi_4_bar
    }
}

/// The gear glyph for the built-in Settings tile.
pub const SETTINGS_GLYPH: &str = "\u{e8b8}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::model::Passphrase;

    #[test]
    fn wifi_effects_map_to_worker_commands() {
        let p = Passphrase::new("hunter22".into());
        let cases = [
            (Effect::SetWifiActive(true), WifiCmd::SetActive(true)),
            (Effect::WifiPowerOn, WifiCmd::PowerOn),
            (Effect::WifiScanning(false), WifiCmd::SetScanning(false)),
            (
                Effect::WifiConnect {
                    path: "/n".into(),
                    passphrase: Some(p.clone()),
                },
                WifiCmd::Connect {
                    path: "/n".into(),
                    passphrase: Some(p),
                },
            ),
            (
                Effect::WifiDisconnect("/n".into()),
                WifiCmd::Disconnect("/n".into()),
            ),
            (
                Effect::WifiForget("/n".into()),
                WifiCmd::Forget("/n".into()),
            ),
        ];
        for (effect, cmd) in cases {
            assert_eq!(wifi_command(&effect), Some(cmd), "{effect:?}");
        }
        assert_eq!(wifi_command(&Effect::PowerOn), None, "Bluetooth's");
        assert_eq!(wifi_command(&Effect::Close), None);
    }
}
