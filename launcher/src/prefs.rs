//! The launcher's own settings (things changed in Settings that no other
//! service stores), kept in tv's home: `$XDG_STATE_HOME/mushtv/settings.toml`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::mirroring::MirrorMode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mirroring_mode: MirrorMode,
}

/// Where the prefs live for the current user.
pub fn default_path() -> Option<PathBuf> {
    default_path_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
}

fn default_path_from(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let state = match xdg_state_home.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|h| !h.is_empty())?).join(".local/state"),
    };
    Some(state.join("mushtv/settings.toml"))
}

/// Reads prefs; a missing or unreadable file means defaults.
pub fn load(path: &Path) -> Prefs {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Prefs::default(),
        Err(e) => {
            tracing::warn!(
                "prefs: couldn't read {}: {e}; using defaults",
                path.display()
            );
            return Prefs::default();
        }
    };
    toml::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!("prefs: {} is invalid ({e}); using defaults", path.display());
        Prefs::default()
    })
}

/// Writes prefs atomically (temporary file, then rename).
pub fn save(path: &Path, prefs: &Prefs) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent directory for {}", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(".settings.toml.tmp");
    std::fs::write(&tmp, toml::to_string(prefs)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn smooth() -> Prefs {
        Prefs {
            mirroring_mode: MirrorMode::Smooth,
        }
    }

    #[test]
    fn missing_file_means_defaults() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("nope.toml")), Prefs::default());
        assert_eq!(Prefs::default().mirroring_mode, MirrorMode::LowLag);
    }

    #[test]
    fn corrupt_file_means_defaults() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        std::fs::write(&p, "mirroring_mode = [[[").unwrap();
        assert_eq!(load(&p), Prefs::default());
        std::fs::write(&p, r#"mirroring_mode = "warp-speed""#).unwrap();
        assert_eq!(load(&p), Prefs::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        std::fs::write(&p, "mirroring_mode = \"smooth\"\nfuture_setting = 3\n").unwrap();
        assert_eq!(load(&p), smooth());
    }

    #[test]
    fn save_then_load_round_trips_and_creates_directories() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state/mushtv/settings.toml");
        save(&p, &smooth()).unwrap();
        assert_eq!(load(&p), smooth());
        save(&p, &Prefs::default()).unwrap();
        assert_eq!(load(&p), Prefs::default());
    }

    #[test]
    fn save_leaves_no_temporary_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        save(&p, &smooth()).unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["settings.toml"]);
    }

    #[test]
    fn save_into_an_unwritable_directory_fails() {
        let d = tempfile::tempdir().unwrap();
        let locked = d.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(save(&locked.join("settings.toml"), &smooth()).is_err());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn path_follows_xdg_state_home_then_home() {
        assert_eq!(
            default_path_from(Some("/x/state".into()), Some("/home/tv".into())),
            Some(PathBuf::from("/x/state/mushtv/settings.toml"))
        );
        assert_eq!(
            default_path_from(None, Some("/home/tv".into())),
            Some(PathBuf::from("/home/tv/.local/state/mushtv/settings.toml"))
        );
        assert_eq!(
            default_path_from(Some("".into()), Some("/home/tv".into())),
            Some(PathBuf::from("/home/tv/.local/state/mushtv/settings.toml"))
        );
        assert_eq!(default_path_from(None, None), None);
    }
}
