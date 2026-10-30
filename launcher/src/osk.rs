//! The on-screen keyboard for typing a Wi-Fi password with a controller or
//! remote: layouts, the highlighted key and the typed text. A physical
//! keyboard types into the same text. Pure, so it's tested without a screen.

/// The longest text accepted: a WPA passphrase is at most 63 characters.
pub const MAX_LEN: usize = 63;

/// The board's width in half-key units (the bottom row is the widest).
pub const BOARD_WIDTH: u8 = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Char(char),
    Shift,
    Symbols,
    Space,
    Delete,
    ShowHide,
    Connect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCap {
    pub kind: KeyKind,
    /// The key's text, or a Material Icons glyph when `icon` is set.
    pub label: String,
    pub icon: bool,
    /// In half-key units: a character key is 2.
    pub width: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedKey {
    pub row: usize,
    /// Left edge, in half-key units from the board's left edge.
    pub x: u8,
    pub cap: KeyCap,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OskOutcome {
    None,
    /// Connect was pressed.
    Submit,
}

const DIGITS: &str = "1234567890";
const LETTERS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const SYMBOLS: [&str; 3] = ["!@#$%^&*()-", "_=+[]{}\\|;:", "'\",.<>/?`~"];
/// The bottom row's index.
const BOTTOM: usize = 4;

/// Its `Debug` hides the typed text, which is a password.
#[derive(Clone, PartialEq, Eq)]
pub struct Osk {
    text: String,
    shift: bool,
    symbols: bool,
    shown: bool,
    row: usize,
    col: usize,
}

impl std::fmt::Debug for Osk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Osk")
            .field(
                "text",
                &format_args!("<{} characters>", self.text.chars().count()),
            )
            .field("shift", &self.shift)
            .field("symbols", &self.symbols)
            .field("shown", &self.shown)
            .field("row", &self.row)
            .field("col", &self.col)
            .finish()
    }
}

impl Default for Osk {
    fn default() -> Self {
        Self::new()
    }
}

impl Osk {
    /// Empty, with the highlight on 'q'.
    pub fn new() -> Self {
        Osk {
            text: String::new(),
            shift: false,
            symbols: false,
            shown: false,
            row: 1,
            col: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// What the field shows: one dot per character, unless shown.
    pub fn shown_text(&self) -> String {
        if self.shown {
            self.text.clone()
        } else {
            "•".repeat(self.text.chars().count())
        }
    }

    #[cfg(test)]
    pub fn shift(&self) -> bool {
        self.shift
    }

    #[cfg(test)]
    pub fn symbols(&self) -> bool {
        self.symbols
    }

    #[cfg(test)]
    pub fn shown(&self) -> bool {
        self.shown
    }

    fn rows(&self) -> Vec<Vec<KeyCap>> {
        let chars = |s: &str| -> Vec<KeyCap> {
            s.chars()
                .map(|c| {
                    let c = if self.shift && !self.symbols {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    };
                    KeyCap {
                        kind: KeyKind::Char(c),
                        label: c.to_string(),
                        icon: false,
                        width: 2,
                    }
                })
                .collect()
        };
        let middle = if self.symbols { SYMBOLS } else { LETTERS };
        let cap = |kind, label: &str, icon, width| KeyCap {
            kind,
            label: label.to_string(),
            icon,
            width,
        };
        vec![
            chars(DIGITS),
            chars(middle[0]),
            chars(middle[1]),
            chars(middle[2]),
            vec![
                cap(KeyKind::Shift, "\u{e318}", true, 3),
                cap(
                    KeyKind::Symbols,
                    if self.symbols { "abc" } else { "!#1" },
                    false,
                    3,
                ),
                cap(KeyKind::Space, "\u{e256}", true, 6),
                cap(KeyKind::Delete, "\u{e14a}", true, 3),
                cap(
                    KeyKind::ShowHide,
                    if self.shown { "\u{e8f5}" } else { "\u{e8f4}" },
                    true,
                    3,
                ),
                cap(KeyKind::Connect, "Connect", false, 4),
            ],
        ]
    }

    /// Every key with its position, rows centred on the board.
    pub fn placed(&self) -> Vec<PlacedKey> {
        let mut keys = vec![];
        for (r, row) in self.rows().into_iter().enumerate() {
            let width: u8 = row.iter().map(|k| k.width).sum();
            let mut x = (BOARD_WIDTH - width) / 2;
            for (c, cap) in row.into_iter().enumerate() {
                let w = cap.width;
                keys.push(PlacedKey {
                    row: r,
                    x,
                    cap,
                    focused: r == self.row && c == self.col,
                });
                x += w;
            }
        }
        keys
    }

    pub fn focus_index(&self) -> usize {
        self.placed().iter().position(|k| k.focused).unwrap_or(0)
    }

    /// Left/Right stop at the row's ends; Up/Down go to the key in the next
    /// row whose centre is nearest (ties go left).
    pub fn move_by(&mut self, dx: i32, dy: i32) {
        let rows = self.rows();
        if dx != 0 {
            let last = rows[self.row].len() - 1;
            self.col = (self.col as i32 + dx).clamp(0, last as i32) as usize;
        }
        if dy != 0 {
            let target = (self.row as i32 + dy).clamp(0, rows.len() as i32 - 1) as usize;
            if target != self.row {
                let placed = self.placed();
                // Centres doubled, to stay in whole numbers.
                let centre2 = |k: &PlacedKey| 2 * k.x as i32 + k.cap.width as i32;
                let here = centre2(&placed[self.focus_index()]);
                let mut best: Option<(i32, usize)> = None;
                for (c, k) in placed.iter().filter(|k| k.row == target).enumerate() {
                    let d = (centre2(k) - here).abs();
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, c));
                    }
                }
                self.row = target;
                self.col = best.map_or(0, |(_, c)| c);
            }
        }
    }

    pub fn press(&mut self) -> OskOutcome {
        let Some(key) = self.placed().into_iter().find(|k| k.focused) else {
            return OskOutcome::None;
        };
        match key.cap.kind {
            KeyKind::Char(c) => self.push(c),
            KeyKind::Space => self.push(' '),
            KeyKind::Shift => self.shift = !self.shift,
            KeyKind::Symbols => {
                self.symbols = !self.symbols;
                self.clamp();
            }
            KeyKind::Delete => self.delete(),
            KeyKind::ShowHide => self.shown = !self.shown,
            KeyKind::Connect => return OskOutcome::Submit,
        }
        OskOutcome::None
    }

    /// A click on a key: highlight it, then press it.
    pub fn press_index(&mut self, i: usize) -> OskOutcome {
        let Some(key) = self.placed().into_iter().nth(i) else {
            return OskOutcome::None;
        };
        self.row = key.row;
        self.col = self
            .placed()
            .iter()
            .take(i)
            .filter(|k| k.row == key.row)
            .count();
        self.press()
    }

    /// A character typed on a physical keyboard. It moves the highlight to
    /// Connect, so Enter (which presses the highlighted key) connects.
    pub fn type_char(&mut self, c: char) {
        if !(' '..='~').contains(&c) {
            return;
        }
        self.push(c);
        self.row = BOTTOM;
        self.col = self.rows()[BOTTOM]
            .iter()
            .position(|k| k.kind == KeyKind::Connect)
            .unwrap_or(0);
    }

    pub fn delete(&mut self) {
        self.text.pop();
    }

    fn push(&mut self, c: char) {
        if self.text.chars().count() < MAX_LEN {
            self.text.push(c);
        }
    }

    /// Keeps the highlight on a key after a layout change.
    fn clamp(&mut self) {
        let len = self.rows()[self.row].len();
        self.col = self.col.min(len - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused(k: &Osk) -> KeyKind {
        k.placed()[k.focus_index()].cap.kind
    }

    /// Moves the highlight onto the key of this kind (searching the board).
    fn go_to(k: &mut Osk, kind: KeyKind) {
        let i = k
            .placed()
            .iter()
            .position(|p| p.cap.kind == kind)
            .unwrap_or_else(|| panic!("no key {kind:?}"));
        let target = k.placed()[i].clone();
        // Walk there with the arrow keys: rows first, then along the row.
        for _ in 0..8 {
            let row = k.placed()[k.focus_index()].row as i32;
            k.move_by(0, (target.row as i32 - row).signum());
        }
        for _ in 0..12 {
            if focused(k) == kind {
                return;
            }
            let here = &k.placed()[k.focus_index()];
            k.move_by((target.x as i32 - here.x as i32).signum(), 0);
        }
        assert_eq!(focused(k), kind);
    }

    fn type_with_keys(k: &mut Osk, text: &str) {
        for c in text.chars() {
            match c {
                ' ' => go_to(k, KeyKind::Space),
                c => go_to(k, KeyKind::Char(c)),
            }
            assert_eq!(k.press(), OskOutcome::None);
        }
    }

    #[test]
    fn starts_on_q_with_empty_hidden_text() {
        let k = Osk::new();
        assert_eq!(focused(&k), KeyKind::Char('q'));
        assert_eq!(k.text(), "");
        assert_eq!(k.shown_text(), "");
        assert!(!k.shift() && !k.symbols() && !k.shown());
    }

    #[test]
    fn pressing_keys_types_them() {
        let mut k = Osk::new();
        k.move_by(1, 0);
        assert_eq!(focused(&k), KeyKind::Char('w'));
        assert_eq!(k.press(), OskOutcome::None);
        assert_eq!(k.text(), "w");
        type_with_keys(&mut k, " ");
        assert_eq!(k.text(), "w ");
    }

    #[test]
    fn shift_types_upper_case_until_pressed_again() {
        let mut k = Osk::new();
        go_to(&mut k, KeyKind::Shift);
        k.press();
        assert!(k.shift());
        assert!(k.placed().iter().any(|p| p.cap.label == "Q"));
        type_with_keys(&mut k, "AB");
        go_to(&mut k, KeyKind::Shift);
        k.press();
        type_with_keys(&mut k, "c");
        assert_eq!(k.text(), "ABc");
    }

    #[test]
    fn symbols_layout_types_punctuation() {
        let mut k = Osk::new();
        go_to(&mut k, KeyKind::Symbols);
        let letters_label = k.placed()[k.focus_index()].cap.label.clone();
        k.press();
        assert!(k.symbols());
        assert_eq!(letters_label, "!#1");
        assert_eq!(k.placed()[k.focus_index()].cap.label, "abc");
        type_with_keys(&mut k, "!@~");
        assert_eq!(k.text(), "!@~");
    }

    #[test]
    fn every_printable_ascii_is_typable() {
        for c in ' '..='~' {
            let typable = [(false, false), (true, false), (false, true), (true, true)]
                .into_iter()
                .any(|(shift, symbols)| {
                    let mut k = Osk::new();
                    if shift {
                        go_to(&mut k, KeyKind::Shift);
                        k.press();
                    }
                    if symbols {
                        go_to(&mut k, KeyKind::Symbols);
                        k.press();
                    }
                    k.placed().iter().any(|p| match p.cap.kind {
                        KeyKind::Char(t) => t == c,
                        KeyKind::Space => c == ' ',
                        _ => false,
                    })
                });
            assert!(typable, "can't type {c:?}");
        }
    }

    #[test]
    fn left_right_stop_at_row_ends() {
        let mut k = Osk::new();
        k.move_by(0, -1);
        assert_eq!(focused(&k), KeyKind::Char('1'));
        k.move_by(-1, 0);
        assert_eq!(focused(&k), KeyKind::Char('1'));
        for _ in 0..12 {
            k.move_by(1, 0);
        }
        assert_eq!(focused(&k), KeyKind::Char('0'));
    }

    #[test]
    fn up_down_go_to_the_nearest_key() {
        let cases = [
            (KeyKind::Char('q'), 1, KeyKind::Char('a')),
            (KeyKind::Char('p'), 1, KeyKind::Char('l')),
            (KeyKind::Char('z'), 1, KeyKind::Symbols),
            (KeyKind::Char('m'), 1, KeyKind::ShowHide),
            (KeyKind::Space, -1, KeyKind::Char('c')),
            (KeyKind::Char('a'), -1, KeyKind::Char('q')), // tie: left wins
            (KeyKind::Char('1'), -1, KeyKind::Char('1')),
            (KeyKind::Connect, 1, KeyKind::Connect),
        ];
        for (from, dy, to) in cases {
            let mut k = Osk::new();
            go_to(&mut k, from);
            k.move_by(0, dy);
            assert_eq!(focused(&k), to, "from {from:?} by {dy}");
        }
    }

    #[test]
    fn rows_of_different_lengths_keep_focus_in_range() {
        let mut k = Osk::new();
        go_to(&mut k, KeyKind::Symbols);
        k.press();
        go_to(&mut k, KeyKind::Char('-')); // row 1's last key (11 keys)
        k.move_by(0, 1);
        assert_eq!(focused(&k), KeyKind::Char(':'));
        k.move_by(0, 1);
        assert_eq!(focused(&k), KeyKind::Char('~')); // row 3 has 10 keys
        go_to(&mut k, KeyKind::Symbols);
        k.press();
        assert_eq!(
            focused(&k),
            KeyKind::Symbols,
            "the switch key keeps the focus"
        );
        assert!(k.focus_index() < k.placed().len());
    }

    #[test]
    fn delete_removes_the_last_character() {
        let mut k = Osk::new();
        k.delete();
        assert_eq!(k.text(), "");
        type_with_keys(&mut k, "ab");
        go_to(&mut k, KeyKind::Delete);
        k.press();
        assert_eq!(k.text(), "a");
        k.delete();
        assert_eq!(k.text(), "");
    }

    #[test]
    fn typing_on_a_keyboard_appends_and_focuses_connect() {
        let mut k = Osk::new();
        k.type_char('x');
        k.type_char('Y');
        k.type_char(' ');
        assert_eq!(k.text(), "xY ");
        assert_eq!(focused(&k), KeyKind::Connect);
    }

    #[test]
    fn non_printable_typed_is_ignored() {
        let mut k = Osk::new();
        for c in ['\u{f700}', 'é', '\n', '\u{8}'] {
            k.type_char(c);
        }
        assert_eq!(k.text(), "");
        assert_eq!(focused(&k), KeyKind::Char('q'));
    }

    #[test]
    fn show_hide_toggles_dots() {
        let mut k = Osk::new();
        type_with_keys(&mut k, "abc");
        assert_eq!(k.shown_text(), "•••");
        go_to(&mut k, KeyKind::ShowHide);
        assert_eq!(k.placed()[k.focus_index()].cap.label, "\u{e8f4}");
        k.press();
        assert!(k.shown());
        assert_eq!(k.shown_text(), "abc");
        assert_eq!(k.placed()[k.focus_index()].cap.label, "\u{e8f5}");
    }

    #[test]
    fn connect_key_submits() {
        let mut k = Osk::new();
        k.type_char('a');
        assert_eq!(k.press(), OskOutcome::Submit);
        assert_eq!(k.text(), "a");
    }

    #[test]
    fn text_is_capped_at_63_characters() {
        let mut k = Osk::new();
        for _ in 0..70 {
            k.type_char('f');
        }
        assert_eq!(k.text().len(), MAX_LEN);
        assert_eq!(MAX_LEN, 63);
    }

    #[test]
    fn debug_hides_the_text() {
        let mut k = Osk::new();
        for c in "hunter22".chars() {
            k.type_char(c);
        }
        let shown = format!("{k:?}");
        assert!(!shown.contains("hunter22"), "{shown}");
    }

    #[test]
    fn press_index_focuses_and_presses() {
        let mut k = Osk::new();
        let e = k
            .placed()
            .iter()
            .position(|p| p.cap.kind == KeyKind::Char('e'))
            .unwrap();
        assert_eq!(k.press_index(e), OskOutcome::None);
        assert_eq!(k.text(), "e");
        assert_eq!(k.focus_index(), e);
    }

    #[test]
    fn keys_fit_the_board_and_icons_are_glyphs() {
        let k = Osk::new();
        for p in k.placed() {
            assert!(
                p.x as u16 + p.cap.width as u16 <= BOARD_WIDTH as u16,
                "{p:?}"
            );
        }
        let bottom: Vec<_> = k.placed().into_iter().filter(|p| p.row == 4).collect();
        let icons: Vec<_> = bottom
            .iter()
            .map(|p| (p.cap.kind, p.cap.icon, p.cap.label.as_str()))
            .collect();
        assert_eq!(
            icons,
            [
                (KeyKind::Shift, true, "\u{e318}"),
                (KeyKind::Symbols, false, "!#1"),
                (KeyKind::Space, true, "\u{e256}"),
                (KeyKind::Delete, true, "\u{e14a}"),
                (KeyKind::ShowHide, true, "\u{e8f4}"),
                (KeyKind::Connect, false, "Connect"),
            ]
        );
        assert_eq!(
            bottom.iter().map(|p| p.cap.width).collect::<Vec<_>>(),
            [3, 3, 6, 3, 3, 4]
        );
    }
}
