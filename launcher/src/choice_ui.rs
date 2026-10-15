//! The picker for a tile with choices (e.g. which GPU to stream Steam from):
//! shown with the Settings page, it starts the chosen command and remembers
//! the choice. Runs on the UI thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::config::Tile;
use crate::input::InputHandle;
use crate::runner::Runner;
use crate::{MainWindow, SettingsRow, choice, prefs};

/// Material Icons "play_arrow", on every row.
const ROW_GLYPH: &str = "\u{e037}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceNav {
    Up,
    Down,
    Select,
    Back,
}

struct Ctl {
    weak: Weak<MainWindow>,
    runner: Arc<Runner>,
    input: Arc<OnceLock<InputHandle>>,
    tiles: Arc<Vec<Tile>>,
    prefs_path: Option<PathBuf>,
    /// (tile index, focused row) while open.
    open: Option<(usize, usize)>,
}

thread_local! {
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ctl) -> R) -> Option<R> {
    CTL.with(|ctl| ctl.borrow_mut().as_mut().map(f))
}

pub fn init(
    ui: &MainWindow,
    runner: Arc<Runner>,
    input: Arc<OnceLock<InputHandle>>,
    tiles: Arc<Vec<Tile>>,
    prefs_path: Option<PathBuf>,
) {
    ui.on_choice_nav(|code| {
        nav(match code {
            0 => ChoiceNav::Up,
            1 => ChoiceNav::Down,
            2 => ChoiceNav::Select,
            _ => ChoiceNav::Back,
        })
    });
    ui.on_choice_hover(|row| {
        with(|ctl| ctl.focus(usize::try_from(row).unwrap_or(0)));
    });
    ui.on_choice_click(|row| {
        with(|ctl| ctl.focus(usize::try_from(row).unwrap_or(0)));
        nav(ChoiceNav::Select);
    });
    CTL.with(|ctl| {
        *ctl.borrow_mut() = Some(Ctl {
            weak: ui.as_weak(),
            runner,
            input,
            tiles,
            prefs_path,
            open: None,
        })
    });
}

pub fn is_open() -> bool {
    with(|ctl| ctl.open.is_some()).unwrap_or(false)
}

/// Opens the picker for `tile`, focused on the choice used last.
pub fn open(tile: usize) {
    with(|ctl| {
        let Some(t) = ctl.tiles.get(tile) else { return };
        let names: Vec<&str> = t.choices.iter().map(|c| c.name.as_str()).collect();
        let remembered = ctl
            .prefs_path
            .as_deref()
            .map(prefs::load)
            .and_then(|p| p.last_choice.get(&t.id).cloned());
        ctl.open = Some((tile, choice::initial_focus(&names, remembered.as_deref())));
        ctl.render();
    });
}

pub fn nav(nav: ChoiceNav) {
    with(|ctl| ctl.nav(nav));
}

impl Ctl {
    fn focus(&mut self, row: usize) {
        if let Some((tile, _)) = self.open {
            let len = self.tiles[tile].choices.len();
            self.open = Some((tile, row.min(len.saturating_sub(1))));
            self.render();
        }
    }

    fn nav(&mut self, nav: ChoiceNav) {
        let Some((tile, focused)) = self.open else {
            return;
        };
        let len = self.tiles[tile].choices.len();
        match nav {
            ChoiceNav::Up => self.open = Some((tile, choice::step(focused, -1, len))),
            ChoiceNav::Down => self.open = Some((tile, choice::step(focused, 1, len))),
            ChoiceNav::Back => self.open = None,
            ChoiceNav::Select => {
                self.open = None;
                self.start(tile, focused);
            }
        }
        self.render();
    }

    fn start(&self, tile: usize, choice: usize) {
        let t = &self.tiles[tile];
        let Some(ui) = self.weak.upgrade() else {
            return;
        };
        if !self.runner.launch_choice(tile, t, choice) {
            return;
        }
        let chosen = &t.choices[choice];
        tracing::info!(
            "opening {} on {} ({})",
            t.name,
            chosen.name,
            chosen.command.join(" ")
        );
        crate::opening(&ui, self.input.get(), &t.name);
        if let Some(path) = &self.prefs_path {
            let (id, name) = (t.id.clone(), chosen.name.clone());
            if let Err(e) = prefs::update(path, |p| {
                p.last_choice.insert(id, name);
            }) {
                tracing::warn!("prefs: {e:#}");
            }
        }
    }

    fn render(&self) {
        let Some(ui) = self.weak.upgrade() else {
            return;
        };
        ui.set_choice_open(self.open.is_some());
        let Some((tile, focused)) = self.open else {
            return;
        };
        let t = &self.tiles[tile];
        let rows: Vec<SettingsRow> = t
            .choices
            .iter()
            .map(|c| SettingsRow {
                icon: ROW_GLYPH.into(),
                label: c.name.as_str().into(),
                detail: "".into(),
                checked: false,
            })
            .collect();
        ui.set_choice_title(t.name.as_str().into());
        ui.set_choice_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        ui.set_choice_focused(i32::try_from(focused).unwrap_or(0));
    }
}
