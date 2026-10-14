//! BlueZ device properties → the launcher's view of a device.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

/// A Bluetooth operation the user started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtOp {
    Pair,
    Connect,
    Disconnect,
    Forget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Speakers, headphones, headsets: can be an audio output.
    Audio,
    Gamepad,
    Keyboard,
    Mouse,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    /// D-Bus object path, e.g. `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`.
    pub path: String,
    /// `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    /// Alias, else Name, else the address.
    pub name: String,
    /// Whether BlueZ knows a real name (scan results without one are hidden).
    pub has_name: bool,
    pub kind: DeviceKind,
    pub paired: bool,
    pub connected: bool,
}

/// Device kind from BlueZ's freedesktop icon name.
pub fn kind_from_icon(icon: Option<&str>) -> DeviceKind {
    match icon {
        Some("audio-card" | "audio-headset" | "audio-headphones" | "audio-speakers") => {
            DeviceKind::Audio
        }
        Some("input-gaming") => DeviceKind::Gamepad,
        Some("input-keyboard") => DeviceKind::Keyboard,
        Some("input-mouse" | "input-tablet") => DeviceKind::Mouse,
        _ => DeviceKind::Other,
    }
}

/// Builds a device from the `org.bluez.Device1` properties at `path`.
pub fn device_from_props(path: &str, props: &HashMap<String, OwnedValue>) -> Option<BtDevice> {
    let string = |key: &str| {
        props
            .get(key)
            .and_then(|v| <&str>::try_from(v).ok())
            .filter(|s| !s.is_empty())
    };
    let flag = |key: &str| {
        props
            .get(key)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    };

    let address = string("Address")?;
    // BlueZ sets Alias to the address with dashes when it knows no name.
    let alias = string("Alias").filter(|a| a.replace('-', ":") != address);
    let name = alias.or_else(|| string("Name"));
    Some(BtDevice {
        path: path.to_string(),
        address: address.to_string(),
        name: name.unwrap_or(address).to_string(),
        has_name: name.is_some(),
        kind: kind_from_icon(string("Icon")),
        paired: flag("Paired"),
        connected: flag("Connected"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn props(entries: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap()))
            .collect()
    }

    #[test]
    fn icons_map_to_kinds() {
        for icon in [
            "audio-card",
            "audio-headset",
            "audio-headphones",
            "audio-speakers",
        ] {
            assert_eq!(kind_from_icon(Some(icon)), DeviceKind::Audio, "{icon}");
        }
        assert_eq!(kind_from_icon(Some("input-gaming")), DeviceKind::Gamepad);
        assert_eq!(kind_from_icon(Some("input-keyboard")), DeviceKind::Keyboard);
        assert_eq!(kind_from_icon(Some("input-mouse")), DeviceKind::Mouse);
        assert_eq!(kind_from_icon(Some("input-tablet")), DeviceKind::Mouse);
        assert_eq!(kind_from_icon(Some("phone")), DeviceKind::Other);
        assert_eq!(kind_from_icon(None), DeviceKind::Other);
    }

    #[test]
    fn full_device() {
        let d = device_from_props(
            "/org/bluez/hci0/dev_00_11_22_AA_BB_CC",
            &props(&[
                ("Address", Value::from("00:11:22:AA:BB:CC")),
                ("Name", Value::from("JBL Flip 5")),
                ("Alias", Value::from("Living room speaker")),
                ("Icon", Value::from("audio-card")),
                ("Paired", Value::from(true)),
                ("Connected", Value::from(true)),
            ]),
        )
        .unwrap();
        assert_eq!(
            d,
            BtDevice {
                path: "/org/bluez/hci0/dev_00_11_22_AA_BB_CC".into(),
                address: "00:11:22:AA:BB:CC".into(),
                name: "Living room speaker".into(),
                has_name: true,
                kind: DeviceKind::Audio,
                paired: true,
                connected: true,
            }
        );
    }

    #[test]
    fn name_falls_back_to_address() {
        let d = device_from_props(
            "/p",
            &props(&[("Address", Value::from("AA:BB:CC:DD:EE:FF"))]),
        )
        .unwrap();
        assert_eq!(d.name, "AA:BB:CC:DD:EE:FF");
        assert!(!d.has_name);
        assert!(!d.paired && !d.connected);
        assert_eq!(d.kind, DeviceKind::Other);
    }

    #[test]
    fn alias_equal_to_dashed_address_is_not_a_name() {
        // BlueZ sets Alias to the address with dashes when there's no name.
        let d = device_from_props(
            "/p",
            &props(&[
                ("Address", Value::from("AA:BB:CC:DD:EE:FF")),
                ("Alias", Value::from("AA-BB-CC-DD-EE-FF")),
            ]),
        )
        .unwrap();
        assert!(!d.has_name);
    }

    #[test]
    fn name_used_when_no_alias() {
        let d = device_from_props(
            "/p",
            &props(&[("Address", Value::from("A")), ("Name", Value::from("Pad"))]),
        )
        .unwrap();
        assert_eq!(d.name, "Pad");
        assert!(d.has_name);
    }

    #[test]
    fn missing_address_is_none() {
        assert_eq!(
            device_from_props("/p", &props(&[("Name", Value::from("X"))])),
            None
        );
    }
}
