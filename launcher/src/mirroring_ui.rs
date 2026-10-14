//! The Mirroring screen: runs UxPlay through the runner while the screen is
//! open, shows the pairing code a new Mac asks for, and closes when the Mac
//! stops mirroring. Runs on the UI thread.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use slint::{ComponentHandle, SharedString, Weak};

use crate::MainWindow;
use crate::input::InputHandle;
use crate::mirroring::{self, MirrorEvent};
use crate::runner::{App, LineSink, Runner};

struct Ctl {
    weak: Weak<MainWindow>,
    runner: Arc<Runner>,
    input: Arc<OnceLock<InputHandle>>,
    name: String,
    open: bool,
}

thread_local! {
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ctl) -> R) -> Option<R> {
    CTL.with(|ctl| ctl.borrow_mut().as_mut().map(f))
}

pub fn init(ui: &MainWindow, runner: Arc<Runner>, input: Arc<OnceLock<InputHandle>>, name: String) {
    ui.set_mirroring_name(name.as_str().into());
    ui.on_mirroring_back(back);
    CTL.with(|ctl| {
        *ctl.borrow_mut() = Some(Ctl {
            weak: ui.as_weak(),
            runner,
            input,
            name,
            open: false,
        })
    });
}

pub fn is_open() -> bool {
    with(|ctl| ctl.open).unwrap_or(false)
}

/// Opens the screen and starts the receiver.
pub fn open() {
    let mode = crate::settings_ui::mirroring_mode();
    with(|ctl| {
        if ctl.open {
            return;
        }
        let weak = ctl.weak.clone();
        let on_line: LineSink = Arc::new(move |line: String| {
            if let Some(event) = mirroring::parse_line(&line) {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with(|ctl| ctl.on_event(event));
                });
            }
        });
        let command = mirroring::command(&ctl.name, mode);
        if ctl
            .runner
            .launch_capturing(App::Mirroring, "mirroring", &command, on_line)
        {
            tracing::info!("mirroring: receiver started ({})", mode.label());
            ctl.open = true;
            if let Some(ui) = ctl.weak.upgrade() {
                ui.set_mirroring_code(SharedString::new());
                ui.set_mirroring_open(true);
            }
        }
    });
}

/// Back on the Mirroring screen: stop the receiver (the screen closes when
/// it has exited).
pub fn back() {
    with(|ctl| {
        if ctl.open {
            ctl.runner.stop();
        }
    });
}

/// The receiver exited, for whatever reason.
pub fn on_exit() {
    with(|ctl| {
        ctl.open = false;
        if let Some(ui) = ctl.weak.upgrade() {
            ui.set_mirroring_open(false);
            ui.set_mirroring_code(SharedString::new());
        }
    });
}

impl Ctl {
    fn on_event(&mut self, event: MirrorEvent) {
        if !self.open {
            return;
        }
        tracing::info!("mirroring: {event:?}");
        let ui = self.weak.upgrade();
        match event {
            MirrorEvent::CodeRequested(code) => {
                if let Some(ui) = ui {
                    ui.set_mirroring_code(code.into());
                }
            }
            MirrorEvent::Connected(_) => {
                if let Some(ui) = ui {
                    ui.set_mirroring_code(SharedString::new());
                }
            }
            // The Mac's picture now covers the screen: from here only Home
            // gets through, like any running app.
            MirrorEvent::Started => {
                if let Some(input) = self.input.get() {
                    input.set_app_active(true);
                }
            }
            MirrorEvent::Stopped => self.runner.stop(),
        }
    }
}
