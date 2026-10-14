//! Connects the Settings state machine to the window, the Bluetooth worker
//! and PipeWire. Everything here runs on the UI thread; worker results are
//! posted back to it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use crate::audio;
use crate::bluetooth::{self, BluetoothHandle, BtCmd, BtEvent};
use crate::settings::{Effect, Icon, Nav, Settings, Snapshot};
use crate::{MainWindow, SettingsRow};

const AUDIO_POLL: Duration = Duration::from_secs(1);

struct Ctl {
    settings: Settings,
    snap: Snapshot,
    bt: BluetoothHandle,
    poll: Timer,
    audio_in_flight: Arc<AtomicBool>,
    weak: Weak<MainWindow>,
}

thread_local! {
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ctl) -> R) -> Option<R> {
    CTL.with(|ctl| ctl.borrow_mut().as_mut().map(f))
}

/// Starts the Bluetooth worker and hooks up the Settings callbacks.
pub fn init(ui: &MainWindow) {
    let weak = ui.as_weak();
    let bt = bluetooth::spawn({
        let weak = weak.clone();
        move |event| {
            let _ = weak.upgrade_in_event_loop(move |_| {
                with(|ctl| ctl.on_bt(event));
            });
        }
    });
    CTL.with(|ctl| {
        *ctl.borrow_mut() = Some(Ctl {
            settings: Settings::new(),
            snap: Snapshot::default(),
            bt,
            poll: Timer::default(),
            audio_in_flight: Arc::new(AtomicBool::new(false)),
            weak,
        })
    });
    ui.on_settings_nav(|code| {
        let nav = match code {
            0 => Nav::Up,
            1 => Nav::Down,
            2 => Nav::Select,
            _ => Nav::Back,
        };
        nav_to(nav);
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
                Effect::SetBtActive(on) => self.bt.send(BtCmd::SetActive(on)),
                Effect::PowerOn => self.bt.send(BtCmd::PowerOn),
                Effect::StartScan => self.bt.send(BtCmd::StartScan),
                Effect::StopScan => self.bt.send(BtCmd::StopScan),
                Effect::Pair(path) => self.bt.send(BtCmd::Pair(path)),
                Effect::Connect(path) => self.bt.send(BtCmd::Connect(path)),
                Effect::Disconnect(path) => self.bt.send(BtCmd::Disconnect(path)),
                Effect::Forget(path) => self.bt.send(BtCmd::Forget(path)),
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
                Effect::Toast(text) => {
                    if let Some(ui) = self.ui() {
                        crate::show_toast(&ui, text);
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
        if self.audio_in_flight.swap(true, Ordering::SeqCst) {
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
            })
            .collect();
        ui.set_settings_title(view.title.into());
        ui.set_settings_status(SharedString::from(view.status));
        ui.set_settings_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        ui.set_settings_focused(i32::try_from(view.focused).unwrap_or(0));
    }
}

/// Material Icons codepoints.
pub fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::AudioOutput => "\u{e050}", // volume_up
        Icon::Bluetooth => "\u{e1a7}",   // bluetooth
        Icon::Tv => "\u{e333}",          // tv
        Icon::Speaker => "\u{e32d}",     // speaker
        Icon::Gamepad => "\u{ea28}",     // sports_esports
        Icon::Keyboard => "\u{e312}",    // keyboard
        Icon::Mouse => "\u{e323}",       // mouse
        Icon::Device => "\u{e337}",      // devices_other
        Icon::Add => "\u{e145}",         // add
        Icon::Connect => "\u{e1aa}",     // bluetooth_searching
        Icon::Disconnect => "\u{e16f}",  // link_off
        Icon::Forget => "\u{e872}",      // delete
    }
}

/// The gear glyph for the built-in Settings tile.
pub const SETTINGS_GLYPH: &str = "\u{e8b8}";
