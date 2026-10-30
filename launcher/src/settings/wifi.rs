//! The Wi-Fi pages: the network list, a network's page and the password
//! keyboard on top of the list.

use std::time::Instant;

use super::{
    Effect, Icon, KeyboardView, Nav, Page, RowView, Settings, Snapshot, WIFI_BUSY_TIMEOUT,
};
use crate::osk::{Osk, OskOutcome};
use crate::wifi::model::{
    Passphrase, Security, StationState, WifiNetwork, WifiOp, signal_bars, valid_passphrase,
};

/// The one Wi-Fi operation in flight.
#[derive(Debug, Clone)]
pub(super) struct WifiBusy {
    op: WifiOp,
    /// The network's path.
    path: String,
    since: Instant,
    /// The keyboard the password was typed on, to reopen if it fails.
    entry: Option<Osk>,
}

/// The password keyboard for one network.
#[derive(Debug, Clone)]
pub(super) struct Popup {
    path: String,
    name: String,
    osk: Osk,
}

impl Settings {
    #[cfg(test)]
    pub fn keyboard_open(&self) -> bool {
        self.popup.is_some()
    }

    /// A click on the keyboard's key `index` (into `KeyboardView::keys`).
    pub fn keyboard_click(&mut self, index: usize, _snap: &Snapshot, now: Instant) -> Vec<Effect> {
        let Some(popup) = self.popup.as_mut() else {
            return vec![];
        };
        match popup.osk.press_index(index) {
            OskOutcome::Submit => self.submit_password(now),
            OskOutcome::None => vec![],
        }
    }

    /// `path`: the network's object path.
    pub fn wifi_finished(
        &mut self,
        op: WifiOp,
        path: &str,
        result: Result<(), String>,
        snap: &Snapshot,
    ) -> Vec<Effect> {
        let ours = self
            .wifi_busy
            .as_ref()
            .is_some_and(|b| b.op == op && b.path == path);
        let entry = if ours {
            self.wifi_busy.take().and_then(|b| b.entry)
        } else {
            None
        };
        if result.is_ok() {
            if ours
                && op == WifiOp::Connect
                && matches!(self.top(), Some(Page::WifiNetwork { path: p }) if p == path)
            {
                self.stack.pop();
            }
            return vec![];
        }
        let name = network_name(snap, path);
        let text = match (op, entry) {
            (WifiOp::Connect, Some(osk)) => {
                // Back to the keyboard, with what was typed, if the list is
                // still what's on screen.
                if self.top() == Some(&Page::Wifi) && self.popup.is_none() {
                    self.popup = Some(Popup {
                        path: path.to_string(),
                        name: name.clone(),
                        osk,
                    });
                }
                format!("Couldn't connect to {name}. Check the password.")
            }
            (WifiOp::Connect, None) => format!("Couldn't connect to {name}"),
            (WifiOp::Disconnect, _) => format!("Couldn't disconnect from {name}"),
            (WifiOp::Forget, _) => format!("Couldn't forget {name}"),
        };
        vec![Effect::Toast(text)]
    }

    pub(super) fn keyboard_nav(&mut self, nav: Nav, now: Instant) -> Vec<Effect> {
        let Some(popup) = self.popup.as_mut() else {
            return vec![];
        };
        match nav {
            Nav::Up => popup.osk.move_by(0, -1),
            Nav::Down => popup.osk.move_by(0, 1),
            Nav::Left => popup.osk.move_by(-1, 0),
            Nav::Right => popup.osk.move_by(1, 0),
            Nav::Delete => popup.osk.delete(),
            Nav::Char(c) => popup.osk.type_char(c),
            Nav::Select => {
                if popup.osk.press() == OskOutcome::Submit {
                    return self.submit_password(now);
                }
            }
            Nav::Submit => return self.submit_password(now),
            Nav::Back => self.popup = None,
            Nav::Home => return self.close(),
        }
        vec![]
    }

    fn submit_password(&mut self, now: Instant) -> Vec<Effect> {
        let Some(popup) = &self.popup else {
            return vec![];
        };
        if !valid_passphrase(popup.osk.text()) {
            return vec![Effect::Toast(
                "Wi-Fi passwords are 8 to 63 characters".into(),
            )];
        }
        if self.wifi_busy.is_some() {
            return vec![];
        }
        let Some(popup) = self.popup.take() else {
            return vec![];
        };
        let passphrase = Passphrase::new(popup.osk.text().to_string());
        self.wifi_busy = Some(WifiBusy {
            op: WifiOp::Connect,
            path: popup.path.clone(),
            since: now,
            entry: Some(popup.osk),
        });
        vec![Effect::WifiConnect {
            path: popup.path,
            passphrase: Some(passphrase),
        }]
    }

    fn start(&mut self, op: WifiOp, path: &str, now: Instant) {
        self.wifi_busy = Some(WifiBusy {
            op,
            path: path.to_string(),
            since: now,
            entry: None,
        });
    }

    pub(super) fn wifi_select(&mut self, snap: &Snapshot, now: Instant) -> Vec<Effect> {
        if !snap.wifi.available {
            return vec![];
        }
        let Some(n) = snap.wifi.networks.get(self.focus()).cloned() else {
            return vec![];
        };
        if n.security == Security::Unsupported {
            return vec![Effect::Toast(format!("{} isn't supported", n.name))];
        }
        if n.known || n.connected {
            self.push(Page::WifiNetwork { path: n.path });
            return vec![];
        }
        match n.security {
            Security::Psk => {
                self.popup = Some(Popup {
                    path: n.path,
                    name: n.name,
                    osk: Osk::new(),
                });
                vec![]
            }
            _ => self.connect_open(&n.path, now),
        }
    }

    /// Connects without a password (open, or saved by iwd).
    fn connect_open(&mut self, path: &str, now: Instant) -> Vec<Effect> {
        if self.wifi_busy.is_some() {
            return vec![];
        }
        self.start(WifiOp::Connect, path, now);
        vec![Effect::WifiConnect {
            path: path.to_string(),
            passphrase: None,
        }]
    }

    pub(super) fn network_select(
        &mut self,
        path: &str,
        snap: &Snapshot,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(n) = find(snap, path) else {
            return vec![];
        };
        match self.focus() {
            0 if self.wifi_busy.is_some() => vec![],
            0 if n.connected => {
                self.start(WifiOp::Disconnect, path, now);
                vec![Effect::WifiDisconnect(path.to_string())]
            }
            0 => self.connect_open(path, now),
            _ => {
                self.stack.pop();
                vec![Effect::WifiForget(path.to_string())]
            }
        }
    }

    /// The network list reorders as signals change: follow the focused
    /// network by key from the start, not only after the first move, so OK
    /// never lands on a network that just slid under the highlight.
    pub(super) fn pin_wifi_focus(&mut self, snap: &Snapshot) {
        if self.top() != Some(&Page::Wifi) {
            return;
        }
        let rows = self.wifi_rows(snap);
        if let Some(frame) = self.stack.last_mut()
            && frame.key.is_none()
        {
            frame.key = rows.get(frame.focus).map(|r| r.key.clone());
        }
    }

    pub(super) fn wifi_on_data(&mut self, snap: &Snapshot, now: Instant) {
        if self
            .wifi_busy
            .as_ref()
            .is_some_and(|b| now >= b.since + WIFI_BUSY_TIMEOUT)
        {
            self.wifi_busy = None;
        }
        // A network page whose network is gone (forgotten, out of range).
        if let Some(Page::WifiNetwork { path }) = self.top()
            && find(snap, path).is_none()
        {
            self.stack.pop();
        }
        self.pin_wifi_focus(snap);
    }

    pub(super) fn wifi_status(&self, snap: &Snapshot) -> String {
        let wifi = &snap.wifi;
        if !wifi.available {
            return "Wi-Fi isn't available".to_string();
        }
        if let Some(b) = self.wifi_busy.as_ref().filter(|b| b.op == WifiOp::Connect) {
            return format!("Connecting to {}…", network_name(snap, &b.path));
        }
        match (&wifi.current, wifi.state) {
            (Some(p), StationState::Connecting) => {
                format!("Connecting to {}…", network_name(snap, p))
            }
            (Some(p), StationState::Connected | StationState::Roaming) => {
                format!("Connected to {}", network_name(snap, p))
            }
            _ if wifi.scanning => "Searching…".to_string(),
            _ => "Not connected".to_string(),
        }
    }

    pub(super) fn wifi_rows(&self, snap: &Snapshot) -> Vec<RowView> {
        if !snap.wifi.available {
            return vec![];
        }
        snap.wifi
            .networks
            .iter()
            .map(|n| {
                let connecting = self
                    .wifi_busy
                    .as_ref()
                    .is_some_and(|b| b.op == WifiOp::Connect && b.path == n.path)
                    || (n.connected && snap.wifi.state == StationState::Connecting);
                let connected = !connecting && is_connected(snap, n);
                let detail = if connecting {
                    "Connecting…"
                } else if connected {
                    "Connected"
                } else if n.security == Security::Unsupported {
                    "Not supported"
                } else if n.known {
                    "Saved"
                } else {
                    ""
                };
                RowView {
                    key: n.path.clone(),
                    icon: Icon::WifiSignal(signal_bars(n.signal)),
                    label: n.name.clone(),
                    detail: detail.to_string(),
                    checked: connected,
                    locked: n.security == Security::Psk,
                }
            })
            .collect()
    }

    pub(super) fn keyboard_view(&self) -> Option<KeyboardView> {
        self.popup.as_ref().map(|p| KeyboardView {
            title: format!("Password for {}", p.name),
            text: p.osk.shown_text(),
            keys: p.osk.placed(),
        })
    }
}

/// Entering the Wi-Fi page: power the device on if needed, and scan.
pub(super) fn enter_effects(snap: &Snapshot) -> Vec<Effect> {
    if !snap.wifi.available {
        return vec![];
    }
    let mut effects = vec![];
    if !snap.wifi.powered {
        effects.push(Effect::WifiPowerOn);
    }
    effects.push(Effect::WifiScanning(true));
    effects
}

/// The Settings menu's Wi-Fi detail.
pub(super) fn menu_detail(snap: &Snapshot) -> String {
    let wifi = &snap.wifi;
    if !wifi.available {
        return "Not available".to_string();
    }
    match (&wifi.current, wifi.state) {
        (Some(_), StationState::Connecting) => "Connecting…".to_string(),
        (Some(p), StationState::Connected | StationState::Roaming) => network_name(snap, p),
        _ => "Not connected".to_string(),
    }
}

pub(super) fn network_title(snap: &Snapshot, path: &str) -> String {
    find(snap, path).map_or_else(|| "Wi-Fi".to_string(), |n| n.name.clone())
}

pub(super) fn network_rows(snap: &Snapshot, path: &str) -> Vec<RowView> {
    let Some(n) = find(snap, path) else {
        return vec![];
    };
    let row = |key: &str, icon, label: &str| RowView {
        key: key.to_string(),
        icon,
        label: label.to_string(),
        detail: String::new(),
        checked: false,
        locked: false,
    };
    vec![
        if n.connected {
            row("link", Icon::Disconnect, "Disconnect")
        } else {
            row("link", Icon::Wifi, "Connect")
        },
        row("forget", Icon::Forget, "Forget"),
    ]
}

fn find<'a>(snap: &'a Snapshot, path: &str) -> Option<&'a WifiNetwork> {
    snap.wifi.networks.iter().find(|n| n.path == path)
}

fn network_name(snap: &Snapshot, path: &str) -> String {
    find(snap, path).map_or_else(|| "the network".to_string(), |n| n.name.clone())
}

fn is_connected(snap: &Snapshot, n: &WifiNetwork) -> bool {
    n.connected
        && matches!(
            snap.wifi.state,
            StationState::Connected | StationState::Roaming
        )
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::wifi::model::{Passphrase, Security, StationState, WifiNetwork, WifiOp, WifiState};

    const HOME: &str = "/net/connman/iwd/0/4/486f6d65_psk";
    const HOTEL: &str = "/net/connman/iwd/0/4/4d617272696f7474_psk";
    const PHONE: &str = "/net/connman/iwd/0/4/50686f6e65_psk";
    const CAFE: &str = "/net/connman/iwd/0/4/43616665_open";
    const CORP: &str = "/net/connman/iwd/0/4/436f7270_8021x";

    fn net(
        path: &str,
        name: &str,
        security: Security,
        signal: i16,
        known: bool,
        connected: bool,
    ) -> WifiNetwork {
        WifiNetwork {
            path: path.into(),
            name: name.into(),
            security,
            signal,
            known,
            connected,
        }
    }

    /// Connected to Home; a hotel network, a saved phone hotspot, an open
    /// café network and an enterprise network nearby, in iwd's order.
    fn snap() -> Snapshot {
        Snapshot {
            wifi: WifiState {
                available: true,
                powered: true,
                scanning: false,
                state: StationState::Connected,
                current: Some(HOME.into()),
                networks: vec![
                    net(HOME, "Home", Security::Psk, -5000, true, true),
                    net(HOTEL, "Marriott-Guest", Security::Psk, -6500, false, false),
                    net(PHONE, "Phone", Security::Psk, -7000, true, false),
                    net(CAFE, "Starbucks", Security::Open, -7200, false, false),
                    net(CORP, "Corp", Security::Unsupported, -8000, false, false),
                ],
            },
            ..Snapshot::default()
        }
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn labels(v: &View) -> Vec<&str> {
        v.rows.iter().map(|r| r.label.as_str()).collect()
    }

    /// Settings open on the Wi-Fi page.
    fn on_wifi(s: &Snapshot) -> Settings {
        let mut st = Settings::new();
        st.open();
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Down, s, now());
        st.nav(Nav::Select, s, now());
        assert_eq!(st.view(s).title, "Wi-Fi");
        st
    }

    /// Focus the Wi-Fi page's row `row` and select it.
    fn pick(st: &mut Settings, s: &Snapshot, row: usize) -> Vec<Effect> {
        st.set_focus(row, s);
        st.nav(Nav::Select, s, now())
    }

    fn type_text(st: &mut Settings, s: &Snapshot, text: &str) {
        for c in text.chars() {
            assert_eq!(st.nav(Nav::Char(c), s, now()), vec![]);
        }
    }

    /// The keyboard for Marriott-Guest, with `text` typed and submitted.
    fn submitted(s: &Snapshot, text: &str) -> (Settings, Vec<Effect>) {
        let mut st = on_wifi(s);
        pick(&mut st, s, 1);
        type_text(&mut st, s, text);
        let effects = st.nav(Nav::Submit, s, now());
        (st, effects)
    }

    #[test]
    fn opening_activates_bluetooth_and_wifi() {
        assert_eq!(
            Settings::new().open(),
            vec![Effect::SetBtActive(true), Effect::SetWifiActive(true)]
        );
    }

    #[test]
    fn menu_has_wifi_row_with_current_network() {
        let s = snap();
        let mut st = Settings::new();
        st.open();
        let row = st.view(&s).rows[2].clone();
        assert_eq!(
            (row.label.as_str(), row.detail.as_str(), row.icon),
            ("Wi-Fi", "Home", Icon::Wifi)
        );
        let mut off = snap();
        off.wifi.available = false;
        assert_eq!(st.view(&off).rows[2].detail, "Not available");
        let mut connecting = snap();
        connecting.wifi.state = StationState::Connecting;
        connecting.wifi.current = Some(HOTEL.into());
        assert_eq!(st.view(&connecting).rows[2].detail, "Connecting…");
        let mut none = snap();
        none.wifi.state = StationState::Disconnected;
        none.wifi.current = None;
        assert_eq!(st.view(&none).rows[2].detail, "Not connected");
    }

    #[test]
    fn menu_selects_by_key() {
        let s = snap();
        let mut st = Settings::new();
        st.open();
        st.set_focus(3, &s);
        st.nav(Nav::Select, &s, now());
        assert_eq!(st.view(&s).title, "Mirroring");
        st.nav(Nav::Back, &s, now());
        st.set_focus(2, &s);
        st.nav(Nav::Select, &s, now());
        assert_eq!(st.view(&s).title, "Wi-Fi");
    }

    #[test]
    fn entering_wifi_powers_on_and_scans() {
        let enter = |s: &Snapshot| {
            let mut st = Settings::new();
            st.open();
            st.set_focus(2, s);
            st.nav(Nav::Select, s, now())
        };
        let mut off = snap();
        off.wifi.powered = false;
        assert_eq!(
            enter(&off),
            vec![Effect::WifiPowerOn, Effect::WifiScanning(true)]
        );
        assert_eq!(enter(&snap()), vec![Effect::WifiScanning(true)]);
        let mut missing = snap();
        missing.wifi.available = false;
        assert_eq!(enter(&missing), vec![]);
    }

    #[test]
    fn leaving_wifi_stops_scanning() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(
            st.nav(Nav::Back, &s, now()),
            vec![Effect::WifiScanning(false)]
        );
        assert_eq!(st.view(&s).title, "Settings");
    }

    #[test]
    fn home_from_wifi_stops_scanning_and_deactivates() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 2); // Phone's page: Wi-Fi is still in the stack
        assert_eq!(
            st.nav(Nav::Home, &s, now()),
            vec![
                Effect::WifiScanning(false),
                Effect::SetBtActive(false),
                Effect::SetWifiActive(false),
                Effect::Close
            ]
        );
        assert!(!st.is_open());
    }

    #[test]
    fn wifi_rows_show_signal_lock_and_status() {
        let s = snap();
        let v = on_wifi(&s).view(&s);
        assert_eq!(
            labels(&v),
            ["Home", "Marriott-Guest", "Phone", "Starbucks", "Corp"]
        );
        let details: Vec<_> = v.rows.iter().map(|r| r.detail.as_str()).collect();
        assert_eq!(details, ["Connected", "", "Saved", "", "Not supported"]);
        let checked: Vec<_> = v.rows.iter().map(|r| r.checked).collect();
        assert_eq!(checked, [true, false, false, false, false]);
        let locked: Vec<_> = v.rows.iter().map(|r| r.locked).collect();
        assert_eq!(locked, [true, true, true, false, false]);
        let icons: Vec<_> = v.rows.iter().map(|r| r.icon).collect();
        assert_eq!(
            icons,
            [
                Icon::WifiSignal(4),
                Icon::WifiSignal(3),
                Icon::WifiSignal(2),
                Icon::WifiSignal(2),
                Icon::WifiSignal(1)
            ]
        );
        assert!(v.keyboard.is_none());
    }

    #[test]
    fn wifi_status_line() {
        let s = snap();
        let st = on_wifi(&s);
        assert_eq!(st.view(&s).status, "Connected to Home");
        let mut searching = snap();
        searching.wifi.state = StationState::Disconnected;
        searching.wifi.current = None;
        searching.wifi.scanning = true;
        assert_eq!(st.view(&searching).status, "Searching…");
        searching.wifi.scanning = false;
        assert_eq!(st.view(&searching).status, "Not connected");
        let mut missing = snap();
        missing.wifi.available = false;
        assert_eq!(st.view(&missing).status, "Wi-Fi isn't available");
        let (st, _) = submitted(&s, "hunter22");
        assert_eq!(st.view(&s).status, "Connecting to Marriott-Guest…");
        assert_eq!(st.view(&s).rows[1].detail, "Connecting…");
    }

    #[test]
    fn unavailable_wifi_page_selects_nothing() {
        let mut s = snap();
        let mut st = on_wifi(&s);
        s.wifi = WifiState::default();
        st.on_data(&s, now());
        assert!(st.view(&s).rows.is_empty());
        assert_eq!(st.nav(Nav::Select, &s, now()), vec![]);
        assert!(!st.keyboard_open());
    }

    #[test]
    fn selecting_a_new_password_network_opens_the_keyboard() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(pick(&mut st, &s, 1), vec![]);
        assert!(st.keyboard_open());
        let kb = st.view(&s).keyboard.expect("keyboard");
        assert_eq!(kb.title, "Password for Marriott-Guest");
        assert_eq!(kb.text, "");
        assert!(!kb.keys.is_empty());
    }

    #[test]
    fn selecting_an_open_network_connects_without_a_password() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(
            pick(&mut st, &s, 3),
            vec![Effect::WifiConnect {
                path: CAFE.into(),
                passphrase: None
            }]
        );
        assert!(!st.keyboard_open());
        assert_eq!(st.view(&s).rows[3].detail, "Connecting…");
    }

    #[test]
    fn selecting_unsupported_shows_toast() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(
            pick(&mut st, &s, 4),
            vec![Effect::Toast("Corp isn't supported".into())]
        );
        assert_eq!(st.view(&s).title, "Wi-Fi");
    }

    #[test]
    fn selecting_saved_or_connected_opens_network_page() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(pick(&mut st, &s, 0), vec![]);
        let v = st.view(&s);
        assert_eq!(v.title, "Home");
        assert_eq!(labels(&v), ["Disconnect", "Forget"]);
        st.nav(Nav::Back, &s, now());
        pick(&mut st, &s, 2);
        let v = st.view(&s);
        assert_eq!(v.title, "Phone");
        assert_eq!(labels(&v), ["Connect", "Forget"]);
        assert_eq!(v.rows[0].icon, Icon::Wifi);
    }

    #[test]
    fn network_page_actions() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 2);
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::WifiConnect {
                path: PHONE.into(),
                passphrase: None
            }]
        );

        let mut st = on_wifi(&s);
        pick(&mut st, &s, 0);
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::WifiDisconnect(HOME.into())]
        );

        let mut st = on_wifi(&s);
        pick(&mut st, &s, 0);
        st.nav(Nav::Down, &s, now());
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::WifiForget(HOME.into())]
        );
        assert_eq!(st.view(&s).title, "Wi-Fi");
    }

    #[test]
    fn keyboard_submit_connects_with_the_typed_password() {
        let s = snap();
        let (st, effects) = submitted(&s, "hunter22");
        assert_eq!(
            effects,
            vec![Effect::WifiConnect {
                path: HOTEL.into(),
                passphrase: Some(Passphrase::new("hunter22".into()))
            }]
        );
        assert!(!st.keyboard_open());
        assert!(st.view(&s).keyboard.is_none());
    }

    #[test]
    fn select_on_connect_key_submits_too() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        type_text(&mut st, &s, "hunter22"); // typing moves the highlight to Connect
        assert_eq!(
            st.nav(Nav::Select, &s, now()),
            vec![Effect::WifiConnect {
                path: HOTEL.into(),
                passphrase: Some(Passphrase::new("hunter22".into()))
            }]
        );
    }

    #[test]
    fn short_password_is_refused_with_toast() {
        let s = snap();
        let (st, effects) = submitted(&s, "short");
        assert_eq!(
            effects,
            vec![Effect::Toast(
                "Wi-Fi passwords are 8 to 63 characters".into()
            )]
        );
        assert!(st.keyboard_open());
        assert_eq!(st.view(&s).keyboard.unwrap().text, "•••••");
    }

    #[test]
    fn failed_connect_reopens_keyboard_with_text_kept() {
        let s = snap();
        let (mut st, _) = submitted(&s, "hunter22");
        assert_eq!(
            st.wifi_finished(
                WifiOp::Connect,
                HOTEL,
                Err("net.connman.iwd.Failed".into()),
                &s
            ),
            vec![Effect::Toast(
                "Couldn't connect to Marriott-Guest. Check the password.".into()
            )]
        );
        let kb = st.view(&s).keyboard.expect("keyboard reopened");
        assert_eq!(kb.title, "Password for Marriott-Guest");
        assert_eq!(kb.text, "••••••••");
    }

    #[test]
    fn failed_open_connect_shows_plain_toast() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 3);
        assert_eq!(
            st.wifi_finished(
                WifiOp::Connect,
                CAFE,
                Err("net.connman.iwd.Failed".into()),
                &s
            ),
            vec![Effect::Toast("Couldn't connect to Starbucks".into())]
        );
        assert!(!st.keyboard_open());
    }

    #[test]
    fn other_failures_name_their_operation() {
        let s = snap();
        let mut st = on_wifi(&s);
        assert_eq!(
            st.wifi_finished(WifiOp::Disconnect, HOME, Err("x".into()), &s),
            vec![Effect::Toast("Couldn't disconnect from Home".into())]
        );
        assert_eq!(
            st.wifi_finished(WifiOp::Forget, PHONE, Err("x".into()), &s),
            vec![Effect::Toast("Couldn't forget Phone".into())]
        );
        assert_eq!(st.wifi_finished(WifiOp::Connect, PHONE, Ok(()), &s), vec![]);
    }

    #[test]
    fn failure_after_leaving_wifi_does_not_reopen_keyboard() {
        let s = snap();
        let (mut st, _) = submitted(&s, "hunter22");
        st.nav(Nav::Home, &s, now());
        let effects = st.wifi_finished(WifiOp::Connect, HOTEL, Err("x".into()), &s);
        assert!(
            matches!(effects.as_slice(), [Effect::Toast(_)]),
            "{effects:?}"
        );
        assert!(!st.is_open());
        assert!(!st.keyboard_open());
        let st = on_wifi(&s);
        assert!(st.view(&s).keyboard.is_none());
    }

    #[test]
    fn back_closes_only_the_keyboard() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        type_text(&mut st, &s, "abc");
        assert_eq!(st.nav(Nav::Back, &s, now()), vec![]);
        assert!(!st.keyboard_open());
        assert_eq!(st.view(&s).title, "Wi-Fi");
        pick(&mut st, &s, 1);
        assert_eq!(
            st.view(&s).keyboard.unwrap().text,
            "",
            "Back discards the text"
        );
    }

    #[test]
    fn home_closes_keyboard_and_settings() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        assert_eq!(
            st.nav(Nav::Home, &s, now()),
            vec![
                Effect::WifiScanning(false),
                Effect::SetBtActive(false),
                Effect::SetWifiActive(false),
                Effect::Close
            ]
        );
        assert!(!st.is_open());
        assert!(!st.keyboard_open());
    }

    #[test]
    fn keyboard_keys_do_nothing_without_keyboard() {
        let s = snap();
        let mut st = on_wifi(&s);
        st.set_focus(1, &s);
        let before = st.view(&s);
        for nav in [
            Nav::Left,
            Nav::Right,
            Nav::Delete,
            Nav::Submit,
            Nav::Char('a'),
        ] {
            assert_eq!(st.nav(nav, &s, now()), vec![], "{nav:?}");
            assert_eq!(st.view(&s), before, "{nav:?}");
        }
    }

    #[test]
    fn keyboard_moves_and_deletes() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        type_text(&mut st, &s, "abc");
        st.nav(Nav::Delete, &s, now());
        assert_eq!(st.view(&s).keyboard.unwrap().text, "••");
        let focus = |st: &Settings| {
            let kb = st.view(&s).keyboard.unwrap();
            kb.keys.iter().position(|k| k.focused).unwrap()
        };
        let at_connect = focus(&st);
        st.nav(Nav::Left, &s, now());
        assert_eq!(focus(&st), at_connect - 1);
        st.nav(Nav::Up, &s, now());
        st.nav(Nav::Right, &s, now());
        assert_ne!(focus(&st), at_connect - 1);
    }

    #[test]
    fn focus_follows_network_when_list_reorders() {
        let mut s = snap();
        let mut st = on_wifi(&s);
        st.nav(Nav::Down, &s, now());
        st.nav(Nav::Down, &s, now()); // Phone
        s.wifi.networks.swap(0, 2); // Phone first now
        st.on_data(&s, now());
        let v = st.view(&s);
        assert_eq!(v.rows[v.focused].label, "Phone");
    }

    #[test]
    fn focus_clamped_when_networks_vanish() {
        let mut s = snap();
        let mut st = on_wifi(&s);
        for _ in 0..4 {
            st.nav(Nav::Down, &s, now());
        }
        s.wifi.networks.truncate(2);
        st.on_data(&s, now());
        let v = st.view(&s);
        assert!(v.focused < 2, "{}", v.focused);
    }

    #[test]
    fn network_page_pops_when_network_disappears() {
        let mut s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 2);
        assert_eq!(st.view(&s).title, "Phone");
        s.wifi.networks.retain(|n| n.path != PHONE);
        st.on_data(&s, now());
        assert_eq!(st.view(&s).title, "Wi-Fi");
    }

    #[test]
    fn wifi_busy_times_out() {
        let s = snap();
        let t = now();
        let mut st = on_wifi(&s);
        st.set_focus(3, &s);
        st.nav(Nav::Select, &s, t); // Starbucks: busy now
        st.set_focus(2, &s);
        st.nav(Nav::Select, &s, t); // Phone's page
        assert_eq!(
            st.nav(Nav::Select, &s, t),
            vec![],
            "one operation at a time"
        );
        st.on_data(&s, t + WIFI_BUSY_TIMEOUT);
        assert_eq!(
            st.nav(Nav::Select, &s, t + WIFI_BUSY_TIMEOUT),
            vec![Effect::WifiConnect {
                path: PHONE.into(),
                passphrase: None
            }]
        );
        assert_eq!(WIFI_BUSY_TIMEOUT, Duration::from_secs(60));
    }

    #[test]
    fn effects_debug_never_shows_password() {
        let s = snap();
        let (_, effects) = submitted(&s, "hunter22");
        let shown = format!("{effects:?}");
        assert!(!shown.contains("hunter22"), "{shown}");
    }

    #[test]
    fn keyboard_click_presses_that_key() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        let keys = st.view(&s).keyboard.unwrap().keys;
        let e = keys
            .iter()
            .position(|k| k.cap.kind == crate::osk::KeyKind::Char('e'))
            .unwrap();
        assert_eq!(st.keyboard_click(e, &s, now()), vec![]);
        assert_eq!(st.view(&s).keyboard.unwrap().text, "•");
        let connect = keys
            .iter()
            .position(|k| k.cap.kind == crate::osk::KeyKind::Connect)
            .unwrap();
        assert_eq!(
            st.keyboard_click(connect, &s, now()),
            vec![Effect::Toast(
                "Wi-Fi passwords are 8 to 63 characters".into()
            )]
        );
    }

    #[test]
    fn focus_follows_network_before_any_move() {
        let mut s = snap();
        let mut st = on_wifi(&s); // focus on Home, never moved
        let home = s.wifi.networks.remove(0);
        s.wifi.networks.insert(2, home);
        st.on_data(&s, now());
        let v = st.view(&s);
        assert_eq!(v.rows[v.focused].label, "Home");
    }

    #[test]
    fn focus_survives_a_wifi_blip() {
        let mut s = snap();
        let mut st = on_wifi(&s);
        st.nav(Nav::Down, &s, now());
        st.nav(Nav::Down, &s, now()); // Phone
        let full = s.wifi.clone();
        s.wifi.networks.clear();
        st.on_data(&s, now());
        s.wifi = full;
        s.wifi.networks.swap(2, 4); // Phone last when the list returns
        st.on_data(&s, now());
        let v = st.view(&s);
        assert_eq!(v.rows[v.focused].label, "Phone");
    }

    #[test]
    fn connect_from_network_page_returns_to_the_list() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 2); // Phone's page
        st.nav(Nav::Select, &s, now()); // Connect
        assert_eq!(st.wifi_finished(WifiOp::Connect, PHONE, Ok(()), &s), vec![]);
        assert_eq!(st.view(&s).title, "Wi-Fi");
    }

    #[test]
    fn settings_debug_never_shows_password() {
        let s = snap();
        let mut st = on_wifi(&s);
        pick(&mut st, &s, 1);
        type_text(&mut st, &s, "hunter22");
        assert!(!format!("{st:?}").contains("hunter22"), "while typing");
        st.nav(Nav::Submit, &s, now());
        assert!(!format!("{st:?}").contains("hunter22"), "while connecting");
    }
}
