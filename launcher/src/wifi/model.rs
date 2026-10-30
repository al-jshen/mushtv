//! iwd's view of Wi-Fi turned into plain data: the station, the networks it
//! sees and how to show them. Pure, so it's tested without iwd.

use std::collections::HashMap;
use std::fmt;

use zbus::zvariant::{ObjectPath, OwnedValue};

pub const IWD_DEVICE: &str = "net.connman.iwd.Device";
pub const IWD_STATION: &str = "net.connman.iwd.Station";
pub const IWD_NETWORK: &str = "net.connman.iwd.Network";

/// Object path → interface → properties, as iwd's ObjectManager reports them.
pub type Objects = HashMap<String, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    Psk,
    /// Enterprise (802.1X) and WEP: listed, but not joinable from the TV.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StationState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Roaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiOp {
    Connect,
    Disconnect,
    Forget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    /// iwd's Network object path.
    pub path: String,
    pub name: String,
    pub security: Security,
    /// dBm × 100, as iwd reports it.
    pub signal: i16,
    /// Saved: iwd has joined it before and will rejoin it by itself.
    pub known: bool,
    /// Connected, or being connected to.
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WifiState {
    /// iwd answers and has a Wi-Fi device.
    pub available: bool,
    pub powered: bool,
    pub scanning: bool,
    pub state: StationState,
    /// The network connected or being connected to (its path).
    pub current: Option<String>,
    /// In iwd's order: connected first, then known, then the rest.
    pub networks: Vec<WifiNetwork>,
}

/// A Wi-Fi password. Its `Debug` never shows it, so it can't leak into a
/// log line or an event dump.
#[derive(Clone, PartialEq, Eq)]
pub struct Passphrase(String);

impl Passphrase {
    pub fn new(text: String) -> Self {
        Passphrase(text)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(<hidden>)")
    }
}

pub fn security_from_type(t: &str) -> Security {
    match t {
        "open" => Security::Open,
        "psk" => Security::Psk,
        _ => Security::Unsupported,
    }
}

pub fn station_state(s: &str) -> StationState {
    match s {
        "connected" => StationState::Connected,
        "connecting" => StationState::Connecting,
        "disconnecting" => StationState::Disconnecting,
        "roaming" => StationState::Roaming,
        _ => StationState::Disconnected,
    }
}

/// 1–4 bars from dBm × 100.
pub fn signal_bars(signal: i16) -> u8 {
    match signal {
        s if s >= -6000 => 4,
        s if s >= -6700 => 3,
        s if s >= -7500 => 2,
        _ => 1,
    }
}

/// A WPA-Personal passphrase: 8–63 characters. (iwd's agent takes only a
/// passphrase; a 64-digit hex key can't be typed in.)
pub fn valid_passphrase(p: &str) -> bool {
    (8..=63).contains(&p.chars().count())
}

fn prop<'a>(props: &'a HashMap<String, OwnedValue>, key: &str) -> Option<&'a OwnedValue> {
    props.get(key)
}

fn string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    prop(props, key).and_then(|v| String::try_from(v.clone()).ok())
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    prop(props, key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn object_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    prop(props, key)
        .and_then(|v| ObjectPath::try_from(v.clone()).ok())
        .map(|p| p.to_string())
}

/// The Wi-Fi device's path: the first object, in path order, with a Device.
pub fn device_path(objects: &Objects) -> Option<String> {
    let mut paths: Vec<&String> = objects
        .iter()
        .filter(|(_, ifaces)| ifaces.contains_key(IWD_DEVICE))
        .map(|(path, _)| path)
        .collect();
    paths.sort();
    paths.first().map(|p| p.to_string())
}

/// `ordered`: `Station.GetOrderedNetworks()`, network paths with signal.
pub fn state_from(objects: &Objects, ordered: &[(String, i16)]) -> WifiState {
    let Some(device) = device_path(objects) else {
        return WifiState::default();
    };
    let ifaces = &objects[&device];
    let powered = flag(&ifaces[IWD_DEVICE], "Powered");
    let Some(station) = ifaces.get(IWD_STATION) else {
        return WifiState {
            available: true,
            powered,
            ..WifiState::default()
        };
    };
    let networks: Vec<WifiNetwork> = ordered
        .iter()
        .filter_map(|(path, signal)| {
            let props = objects.get(path)?.get(IWD_NETWORK)?;
            let name = string(props, "Name").filter(|n| !n.is_empty())?;
            Some(WifiNetwork {
                path: path.clone(),
                name,
                security: security_from_type(&string(props, "Type").unwrap_or_default()),
                signal: *signal,
                known: object_path(props, "KnownNetwork").is_some(),
                connected: flag(props, "Connected"),
            })
        })
        .collect();
    let current = object_path(station, "ConnectedNetwork").or_else(|| {
        networks
            .iter()
            .find(|n| n.connected)
            .map(|n| n.path.clone())
    });
    WifiState {
        available: true,
        powered,
        scanning: flag(station, "Scanning"),
        state: station_state(&string(station, "State").unwrap_or_default()),
        current,
        networks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};

    const DEV: &str = "/net/connman/iwd/0/4";
    const HOME: &str = "/net/connman/iwd/0/4/486f6d65_psk";
    const HOTEL: &str = "/net/connman/iwd/0/4/486f74656c_psk";
    const CAFE: &str = "/net/connman/iwd/0/4/43616665_open";

    fn props(entries: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap()))
            .collect()
    }

    fn path(p: &'static str) -> Value<'static> {
        Value::from(ObjectPath::try_from(p).unwrap())
    }

    fn network(
        name: &str,
        kind: &str,
        connected: bool,
        known: Option<&'static str>,
    ) -> HashMap<String, OwnedValue> {
        let mut entries = vec![
            ("Name", Value::from(name.to_string())),
            ("Type", Value::from(kind.to_string())),
            ("Connected", Value::from(connected)),
            ("Device", path(DEV)),
        ];
        if let Some(k) = known {
            entries.push(("KnownNetwork", path(k)));
        }
        props(&entries)
    }

    fn objects() -> Objects {
        let mut o = Objects::new();
        o.insert(
            DEV.into(),
            HashMap::from([
                (
                    IWD_DEVICE.to_string(),
                    props(&[
                        ("Powered", Value::from(true)),
                        ("Name", Value::from("wlp2s0")),
                    ]),
                ),
                (
                    IWD_STATION.to_string(),
                    props(&[
                        ("State", Value::from("connected")),
                        ("Scanning", Value::from(false)),
                        ("ConnectedNetwork", path(HOME)),
                    ]),
                ),
            ]),
        );
        o.insert(
            HOME.into(),
            HashMap::from([(
                IWD_NETWORK.to_string(),
                network("Home", "psk", true, Some("/net/connman/iwd/486f6d65_psk")),
            )]),
        );
        o.insert(
            HOTEL.into(),
            HashMap::from([(
                IWD_NETWORK.to_string(),
                network("Marriott-Guest", "psk", false, None),
            )]),
        );
        o.insert(
            CAFE.into(),
            HashMap::from([(
                IWD_NETWORK.to_string(),
                network("Starbucks", "open", false, None),
            )]),
        );
        o
    }

    fn ordered() -> Vec<(String, i16)> {
        vec![
            (HOME.into(), -5000),
            (HOTEL.into(), -6500),
            (CAFE.into(), -7200),
        ]
    }

    #[test]
    fn security_maps_iwd_types() {
        assert_eq!(security_from_type("open"), Security::Open);
        assert_eq!(security_from_type("psk"), Security::Psk);
        for t in ["8021x", "wep", "x"] {
            assert_eq!(security_from_type(t), Security::Unsupported, "{t}");
        }
    }

    #[test]
    fn station_state_strings() {
        assert_eq!(station_state("connected"), StationState::Connected);
        assert_eq!(station_state("connecting"), StationState::Connecting);
        assert_eq!(station_state("disconnecting"), StationState::Disconnecting);
        assert_eq!(station_state("roaming"), StationState::Roaming);
        assert_eq!(station_state("disconnected"), StationState::Disconnected);
        assert_eq!(station_state(""), StationState::Disconnected);
    }

    #[test]
    fn signal_bars_by_strength() {
        for (signal, bars) in [
            (-5500, 4),
            (-6000, 4),
            (-6100, 3),
            (-6700, 3),
            (-7000, 2),
            (-7500, 2),
            (-7600, 1),
            (-9000, 1),
        ] {
            assert_eq!(signal_bars(signal), bars, "{signal}");
        }
    }

    #[test]
    fn passphrase_length_rules() {
        assert!(!valid_passphrase("1234567"));
        assert!(valid_passphrase("12345678"));
        assert!(valid_passphrase(&"a".repeat(63)));
        // iwd takes a typed password through its agent only as a passphrase
        // (8-63 characters), never as a 64-digit hex key.
        assert!(!valid_passphrase(&"z".repeat(64)));
        assert!(!valid_passphrase(&"f".repeat(64)));
        assert!(!valid_passphrase(""));
    }

    #[test]
    fn passphrase_debug_hides_text() {
        let shown = format!("{:?}", Passphrase::new("hunter22".into()));
        assert!(!shown.contains("hunter22"), "{shown}");
        assert_eq!(Passphrase::new("hunter22".into()).expose(), "hunter22");
    }

    #[test]
    fn state_lists_networks_in_iwd_order() {
        let s = state_from(&objects(), &ordered());
        assert!(s.available && s.powered && !s.scanning);
        assert_eq!(s.state, StationState::Connected);
        assert_eq!(s.current.as_deref(), Some(HOME));
        let names: Vec<_> = s.networks.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Home", "Marriott-Guest", "Starbucks"]);
        assert_eq!(
            s.networks.iter().map(|n| n.known).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            s.networks.iter().map(|n| n.connected).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            s.networks.iter().map(|n| n.signal).collect::<Vec<_>>(),
            [-5000, -6500, -7200]
        );
        assert_eq!(
            s.networks.iter().map(|n| n.security).collect::<Vec<_>>(),
            [Security::Psk, Security::Psk, Security::Open]
        );
        assert_eq!(s.networks[1].path, HOTEL);
    }

    #[test]
    fn nameless_networks_are_dropped() {
        let mut o = objects();
        o.insert(
            "/net/connman/iwd/0/4/_psk".into(),
            HashMap::from([(IWD_NETWORK.to_string(), network("", "psk", false, None))]),
        );
        let mut order = ordered();
        order.push(("/net/connman/iwd/0/4/_psk".into(), -4000));
        let s = state_from(&o, &order);
        assert_eq!(s.networks.len(), 3);
    }

    #[test]
    fn no_device_means_unavailable() {
        assert_eq!(state_from(&Objects::new(), &[]), WifiState::default());
        assert!(!WifiState::default().available);
    }

    #[test]
    fn powered_off_device_is_available_but_empty() {
        let mut o = Objects::new();
        o.insert(
            DEV.into(),
            HashMap::from([(
                IWD_DEVICE.to_string(),
                props(&[("Powered", Value::from(false))]),
            )]),
        );
        let s = state_from(&o, &[]);
        assert!(s.available);
        assert!(!s.powered);
        assert!(s.networks.is_empty());
        assert_eq!(s.state, StationState::Disconnected);
    }
}
