//! Canned Settings data for screenshots. With `MUSHTV_DEMO` set, Settings
//! shows this and never talks to BlueZ, PipeWire or iwd.

use crate::audio::{AudioState, Output, OutputKind};
use crate::bluetooth::model::{BtDevice, DeviceKind};
use crate::mirroring::MirrorMode;
use crate::settings::Snapshot;
use crate::wifi::model::{Security, StationState, WifiNetwork, WifiState};

pub fn snapshot(mirroring_mode: MirrorMode) -> Snapshot {
    let device = |address: &str, name: &str, kind, connected| BtDevice {
        path: format!("/org/bluez/hci0/dev_{}", address.replace(':', "_")),
        address: address.into(),
        name: name.into(),
        has_name: true,
        kind,
        paired: true,
        connected,
    };
    let network = |id: &str, name: &str, security, signal, known, connected| WifiNetwork {
        path: format!("/net/connman/iwd/0/4/{id}"),
        name: name.into(),
        security,
        signal,
        known,
        connected,
    };
    Snapshot {
        bt_available: true,
        powered: true,
        scanning: false,
        devices: vec![
            device("00:11:22:AA:BB:CC", "JBL Flip 5", DeviceKind::Audio, false),
            device(
                "11:22:33:44:55:66",
                "8BitDo Ultimate 2C",
                DeviceKind::Gamepad,
                true,
            ),
        ],
        audio: Some(AudioState {
            outputs: vec![Output {
                id: 52,
                name: "alsa_output.pci-0000_00_1f.3.hdmi-stereo".into(),
                description: "HDMI".into(),
                kind: OutputKind::Tv,
                bt_address: None,
            }],
            configured: None,
        }),
        passkey: None,
        mirroring_mode,
        wifi: WifiState {
            available: true,
            powered: true,
            scanning: false,
            state: StationState::Connected,
            current: Some("/net/connman/iwd/0/4/486f6d65_psk".into()),
            networks: vec![
                network("486f6d65_psk", "Home", Security::Psk, -5000, true, true),
                network(
                    "4d617272696f7474_psk",
                    "Marriott-Guest",
                    Security::Psk,
                    -6500,
                    false,
                    false,
                ),
                network("50686f6e65_psk", "Phone", Security::Psk, -7000, true, false),
                network(
                    "53746172_open",
                    "Starbucks",
                    Security::Open,
                    -7200,
                    false,
                    false,
                ),
                network(
                    "436f7270_8021x",
                    "Corp",
                    Security::Unsupported,
                    -8000,
                    false,
                    false,
                ),
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::model::Security;

    #[test]
    fn demo_snapshot_has_every_kind_of_network() {
        let s = snapshot(MirrorMode::LowLag);
        assert!(s.bt_available && s.powered);
        assert!(s.devices.iter().any(|d| d.paired && d.connected));
        assert!(s.audio.is_some());
        let wifi = &s.wifi;
        assert!(wifi.available && wifi.powered);
        assert!(wifi.networks.iter().any(|n| n.connected));
        assert!(wifi.networks.iter().any(|n| n.known && !n.connected));
        for security in [Security::Psk, Security::Open, Security::Unsupported] {
            assert!(
                wifi.networks.iter().any(|n| n.security == security),
                "{security:?}"
            );
        }
    }
}
