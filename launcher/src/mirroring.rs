//! Screen mirroring from a Mac (AirPlay) via UxPlay: the command to run and
//! the meaning of UxPlay's output lines.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MirrorMode {
    /// No audio/video timestamp sync: responsive, video may hitch.
    #[default]
    LowLag,
    /// UxPlay's default sync: smoother video, about half a second of lag.
    Smooth,
}

impl MirrorMode {
    pub fn label(self) -> &'static str {
        match self {
            MirrorMode::LowLag => "Low lag",
            MirrorMode::Smooth => "Smooth playback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorEvent {
    /// A Mac is pairing for the first time and must enter this code.
    CodeRequested(String),
    /// A Mac (by name) has been accepted.
    Connected(String),
    /// The Mac's picture is now on screen.
    Started,
    /// Mirroring ended (stopped on the Mac, or it disconnected).
    Stopped,
}

/// The UxPlay command for this receiver name and mode.
///
/// `stdbuf` makes UxPlay's output line-buffered: piped, it would otherwise
/// sit in a buffer and the code prompt would never arrive in time.
pub fn command(name: &str, mode: MirrorMode) -> Vec<String> {
    let mut cmd: Vec<String> = [
        "stdbuf",
        "-oL",
        "-eL",
        "uxplay",
        "-n",
        name,
        "-nh",
        // Fixed ports, full screen, GPU decoding and conversion straight to
        // the display (frames stay on the GPU).
        "-p",
        "-fs",
        "-vs",
        "waylandsink",
        "-vd",
        "vah264dec",
        "-vc",
        "vapostproc",
        // The TV panel is 1080p.
        "-s",
        "1920x1080@30",
        "-fps",
        "30",
        // New Macs must enter the code shown on the TV.
        "-pin",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if mode == MirrorMode::LowLag {
        cmd.extend(["-vsync".to_string(), "no".to_string()]);
    }
    cmd
}

/// What a line of UxPlay's output means, if anything we act on.
pub fn parse_line(line: &str) -> Option<MirrorEvent> {
    let line = line.trim();
    if line.contains("CLIENT MUST NOW ENTER PIN") {
        let code = line.split('"').nth(1)?;
        return (!code.is_empty() && code.chars().all(|c| c.is_ascii_digit()))
            .then(|| MirrorEvent::CodeRequested(code.to_string()));
    }
    if let Some(rest) = line.strip_prefix("connection request from ") {
        let who = rest.split(" with deviceID").next()?.trim();
        // UxPlay prints "<name> (<model>)"; drop the model.
        let name = match who.rfind(" (") {
            Some(i) if who.ends_with(')') => &who[..i],
            _ => who,
        };
        return Some(MirrorEvent::Connected(name.to_string()));
    }
    match line {
        "raop_rtp_mirror starting mirroring" => Some(MirrorEvent::Started),
        "raop_rtp_mirror->running is no longer true" => Some(MirrorEvent::Stopped),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/uxplay-session.log"
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn low_lag_command() {
        assert_eq!(
            command("mushtv", MirrorMode::LowLag),
            [
                "stdbuf",
                "-oL",
                "-eL",
                "uxplay",
                "-n",
                "mushtv",
                "-nh",
                "-p",
                "-fs",
                "-vs",
                "waylandsink",
                "-vd",
                "vah264dec",
                "-vc",
                "vapostproc",
                "-s",
                "1920x1080@30",
                "-fps",
                "30",
                "-pin",
                "-vsync",
                "no",
            ]
        );
    }

    #[test]
    fn smooth_command_keeps_uxplays_sync() {
        let cmd = command("Living room", MirrorMode::Smooth);
        assert!(!cmd.iter().any(|a| a == "-vsync"), "{cmd:?}");
        let name = cmd.iter().position(|a| a == "-n").unwrap();
        assert_eq!(cmd[name + 1], "Living room");
        assert!(cmd.iter().any(|a| a == "-pin"));
    }

    #[test]
    fn a_real_session_gives_exactly_these_events() {
        let events: Vec<_> = fixture().lines().filter_map(parse_line).collect();
        assert_eq!(
            events,
            [
                MirrorEvent::CodeRequested("7932".into()),
                MirrorEvent::Connected("Living Room Mac".into()),
                MirrorEvent::Started,
                MirrorEvent::Stopped,
            ]
        );
    }

    #[test]
    fn probes_and_connection_noise_mean_nothing() {
        for line in [
            "Accepted IPv4 client on socket 27",
            "ignoring AirPlay video streaming request (use option -hls to activate HLS support)",
            "*** WARNING: httpd didn't get response",
            "Connection closed for socket 27",
            "client sent PAIR-PIN-START request",
            "",
            "PIN",
        ] {
            assert_eq!(parse_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn code_line_variants() {
        assert_eq!(
            parse_line(r#"*** CLIENT MUST NOW ENTER PIN = "0042" AS AIRPLAY PASSWORD"#),
            Some(MirrorEvent::CodeRequested("0042".into()))
        );
        assert_eq!(
            parse_line(r#"  *** CLIENT MUST NOW ENTER PIN = "1234" AS AIRPLAY PASSWORD  "#),
            Some(MirrorEvent::CodeRequested("1234".into()))
        );
        assert_eq!(
            parse_line(r#"*** CLIENT MUST NOW ENTER PIN = "" AS AIRPLAY PASSWORD"#),
            None
        );
    }

    #[test]
    fn connection_line_without_model_still_names_the_mac() {
        assert_eq!(
            parse_line("connection request from Jeff's MacBook with deviceID = 11:22:33:44:55:66"),
            Some(MirrorEvent::Connected("Jeff's MacBook".into()))
        );
    }

    #[test]
    fn mode_is_saved_as_kebab_case() {
        #[derive(Serialize, Deserialize)]
        struct W {
            m: MirrorMode,
        }
        assert_eq!(
            toml::to_string(&W {
                m: MirrorMode::LowLag
            })
            .unwrap()
            .trim(),
            r#"m = "low-lag""#
        );
        let w: W = toml::from_str(r#"m = "smooth""#).unwrap();
        assert_eq!(w.m, MirrorMode::Smooth);
    }
}
