//! Audio outputs from PipeWire: which sinks exist, which one the user chose,
//! and switching the choice. Talks to the session through `pw-dump` and
//! `wpctl`; WirePlumber stores the choice and falls back to the TV when a
//! chosen speaker is gone.

use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// The TV over HDMI.
    Tv,
    Bluetooth,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// PipeWire object id, for `wpctl set-default`.
    pub id: u32,
    /// `node.name`, stable across restarts.
    pub name: String,
    pub description: String,
    pub kind: OutputKind,
    /// `AA:BB:CC:DD:EE:FF` for Bluetooth outputs.
    pub bt_address: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioState {
    pub outputs: Vec<Output>,
    /// The user's stored choice (`default.configured.audio.sink`), a node name.
    pub configured: Option<String>,
}

/// Parses `pw-dump` output.
pub fn parse(json: &str) -> Result<AudioState> {
    let objects: Vec<Value> =
        serde_json::from_str(json).context("pw-dump output isn't a JSON array")?;
    let mut state = AudioState::default();
    for object in &objects {
        let kind = object["type"].as_str().unwrap_or_default();
        if kind.ends_with(":Node") {
            if let Some(output) = output_from_node(object) {
                state.outputs.push(output);
            }
        } else if kind.ends_with(":Metadata") && object["props"]["metadata.name"] == "default" {
            state.configured = object["metadata"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|entry| entry["key"] == "default.configured.audio.sink")
                .and_then(|entry| entry["value"]["name"].as_str())
                .map(str::to_string);
        }
    }
    Ok(state)
}

fn output_from_node(node: &Value) -> Option<Output> {
    let props = &node["info"]["props"];
    if props["media.class"] != "Audio/Sink" {
        return None;
    }
    let name = props["node.name"].as_str()?.to_string();
    let kind = match props["device.api"].as_str() {
        Some("bluez5") => OutputKind::Bluetooth,
        Some("alsa") if name.contains("hdmi") => OutputKind::Tv,
        _ => OutputKind::Other,
    };
    let bt_address = match kind {
        OutputKind::Bluetooth => props["api.bluez5.address"]
            .as_str()
            .map(str::to_string)
            .or_else(|| bt_address_from_node_name(&name)),
        _ => None,
    };
    Some(Output {
        id: u32::try_from(node["id"].as_u64()?).ok()?,
        description: props["node.description"]
            .as_str()
            .unwrap_or(&name)
            .to_string(),
        name,
        kind,
        bt_address,
    })
}

/// `bluez_output.AA_BB_CC_DD_EE_FF.1` → `AA:BB:CC:DD:EE:FF`.
pub fn bt_address_from_node_name(name: &str) -> Option<String> {
    let mac = name.strip_prefix("bluez_output.")?.split('.').next()?;
    let parts: Vec<&str> = mac.split('_').collect();
    let valid = parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
    valid.then(|| parts.join(":").to_ascii_uppercase())
}

/// Reads the current outputs from the running PipeWire session.
pub fn read() -> Result<AudioState> {
    let out = Command::new("pw-dump")
        .output()
        .context("couldn't run pw-dump")?;
    if !out.status.success() {
        bail!(
            "pw-dump failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Makes `id` the chosen output. WirePlumber remembers the choice.
pub fn set_default(id: u32) -> Result<()> {
    let out = Command::new("wpctl")
        .args(["set-default", &id.to_string()])
        .output()
        .context("couldn't run wpctl")?;
    if !out.status.success() {
        bail!(
            "wpctl set-default {id} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes taken from this machine's `pw-dump`, plus a Bluetooth speaker.
    const DUMP: &str = r#"[
      { "id": 41, "type": "PipeWire:Interface:Metadata",
        "props": { "metadata.name": "default" },
        "metadata": [
          { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON",
            "value": { "name": "alsa_output.pci-0000_00_1f.3.hdmi-stereo" } },
          { "subject": 0, "key": "default.configured.audio.sink", "type": "Spa:String:JSON",
            "value": { "name": "bluez_output.00_11_22_AA_BB_CC.1" } }
        ] },
      { "id": 32, "type": "PipeWire:Interface:Metadata",
        "props": { "metadata.name": "settings" },
        "metadata": [ { "subject": 0, "key": "clock.rate", "type": "", "value": 48000 } ] },
      { "id": 52, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "device.api": "alsa", "media.class": "Audio/Sink",
          "node.description": "Built-in Audio Digital Stereo (HDMI)",
          "node.name": "alsa_output.pci-0000_00_1f.3.hdmi-stereo" } } },
      { "id": 53, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "device.api": "alsa", "media.class": "Audio/Source",
          "node.description": "Built-in Audio Analog Stereo",
          "node.name": "alsa_input.pci-0000_00_1f.3.analog-stereo" } } },
      { "id": 77, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "device.api": "bluez5", "media.class": "Audio/Sink",
          "api.bluez5.address": "00:11:22:AA:BB:CC",
          "node.description": "JBL Flip 5",
          "node.name": "bluez_output.00_11_22_AA_BB_CC.1" } } },
      { "id": 90, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "media.class": "Stream/Output/Audio",
          "node.name": "mpv" } } },
      { "id": 95, "type": "PipeWire:Interface:Client", "info": { "props": {} } }
    ]"#;

    #[test]
    fn parses_sinks_and_ignores_everything_else() {
        let s = parse(DUMP).unwrap();
        let names: Vec<_> = s.outputs.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "alsa_output.pci-0000_00_1f.3.hdmi-stereo",
                "bluez_output.00_11_22_AA_BB_CC.1"
            ]
        );
    }

    #[test]
    fn classifies_outputs() {
        let s = parse(DUMP).unwrap();
        assert_eq!(s.outputs[0].kind, OutputKind::Tv);
        assert_eq!(s.outputs[0].id, 52);
        assert_eq!(s.outputs[0].bt_address, None);
        assert_eq!(s.outputs[1].kind, OutputKind::Bluetooth);
        assert_eq!(s.outputs[1].description, "JBL Flip 5");
        assert_eq!(
            s.outputs[1].bt_address.as_deref(),
            Some("00:11:22:AA:BB:CC")
        );
    }

    #[test]
    fn analog_sink_is_other() {
        let dump = r#"[{ "id": 51, "type": "PipeWire:Interface:Node", "info": { "props": {
            "device.api": "alsa", "media.class": "Audio/Sink",
            "node.description": "Built-in Audio Analog Stereo",
            "node.name": "alsa_output.pci-0000_00_1f.3.analog-stereo" } } }]"#;
        assert_eq!(parse(dump).unwrap().outputs[0].kind, OutputKind::Other);
    }

    #[test]
    fn reads_the_configured_choice() {
        assert_eq!(
            parse(DUMP).unwrap().configured.as_deref(),
            Some("bluez_output.00_11_22_AA_BB_CC.1")
        );
    }

    #[test]
    fn no_configured_choice_is_none() {
        let dump = r#"[{ "id": 41, "type": "PipeWire:Interface:Metadata",
            "props": { "metadata.name": "default" },
            "metadata": [ { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON",
              "value": { "name": "x" } } ] }]"#;
        assert_eq!(parse(dump).unwrap().configured, None);
    }

    #[test]
    fn bluetooth_address_falls_back_to_the_node_name() {
        let dump = r#"[{ "id": 7, "type": "PipeWire:Interface:Node", "info": { "props": {
            "device.api": "bluez5", "media.class": "Audio/Sink",
            "node.description": "Speaker", "node.name": "bluez_output.AA_BB_CC_DD_EE_FF.1" } } }]"#;
        assert_eq!(
            parse(dump).unwrap().outputs[0].bt_address.as_deref(),
            Some("AA:BB:CC:DD:EE:FF")
        );
    }

    #[test]
    fn node_name_to_address() {
        assert_eq!(
            bt_address_from_node_name("bluez_output.00_11_22_AA_BB_CC.1").as_deref(),
            Some("00:11:22:AA:BB:CC")
        );
        assert_eq!(
            bt_address_from_node_name("bluez_output.00_11_22_AA_BB_CC").as_deref(),
            Some("00:11:22:AA:BB:CC")
        );
        assert_eq!(
            bt_address_from_node_name("alsa_output.pci-0000_00_1f.3.hdmi-stereo"),
            None
        );
        assert_eq!(bt_address_from_node_name("bluez_output.nonsense.1"), None);
        assert_eq!(bt_address_from_node_name("bluez_output."), None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse("not json").is_err());
        assert!(parse("{}").is_err());
    }
}

#[cfg(test)]
mod live {
    /// Needs a running PipeWire session: `cargo test -- --ignored live`.
    #[test]
    #[ignore]
    fn reads_the_live_session() {
        let state = super::read().unwrap();
        assert!(!state.outputs.is_empty(), "{state:?}");
        eprintln!("{state:#?}");
    }
}
