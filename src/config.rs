//! Application configuration persisted as TOML.
//!
//! Stored at `~/Library/Application Support/minisnap/config.toml`; created
//! with defaults on first run. Hotkeys are stored in the `global-hotkey`
//! string form (`alt+meta+Digit5`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Global hotkey that starts the frozen-area capture.
    pub capture_hotkey: HotKey,
    /// Global hotkey that starts the capture-and-edit mode.
    pub capture_edit_hotkey: HotKey,
    /// Global hotkey that opens the latest saved shot.
    pub open_hotkey: HotKey,
    /// Directory where shots are saved silently.
    pub save_dir: PathBuf,
    /// Maximum number of saved shots kept on disk.
    pub retention: u32,
}

/// The pre-editor default capture combination (⌘⌥5). configs written before
/// the editor existed may still carry it for `capture_hotkey`.
pub fn legacy_capture_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit5)
}

pub fn default_capture_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit4)
}

pub fn default_capture_edit_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit5)
}

pub fn default_open_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit0)
}

impl Config {
    /// Defaults with the shots folder inside the user's pictures directory.
    pub fn default_with(pictures_dir: PathBuf) -> Config {
        Config {
            capture_hotkey: default_capture_hotkey(),
            capture_edit_hotkey: default_capture_edit_hotkey(),
            open_hotkey: default_open_hotkey(),
            save_dir: pictures_dir.join("minisnap"),
            retention: 100,
        }
    }

    /// Standard config file location, if the system dirs are known.
    pub fn standard_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("minisnap").join("config.toml"))
    }

    pub fn load_from(path: &Path) -> io::Result<Config> {
        Self::load_from_migrating(path).map(|(config, _)| config)
    }

    /// Like `load_from`, also reporting whether the one-time legacy-defaults
    /// migration fired (so the caller can persist the shifted combinations).
    pub fn load_from_migrating(path: &Path) -> io::Result<(Config, bool)> {
        let text = fs::read_to_string(path)?;
        let raw: RawConfig =
            toml::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut config = Config {
            capture_edit_hotkey: raw
                .capture_edit_hotkey
                .unwrap_or_else(default_capture_edit_hotkey),
            capture_hotkey: raw.capture_hotkey,
            open_hotkey: raw.open_hotkey,
            save_dir: raw.save_dir,
            retention: raw.retention,
        };
        let migrated = migrate_legacy_defaults(&mut config, raw.capture_edit_hotkey.is_some());
        Ok((config, migrated))
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, text)
    }

    /// Load from the standard location; create with defaults on first run.
    /// A corrupt file falls back to defaults (kept in memory only — the file
    /// is not overwritten so nothing is lost). A one-time legacy-defaults
    /// migration writes the shifted combinations back.
    pub fn load_or_create() -> Config {
        let Some(path) = Self::standard_path() else {
            eprintln!("minisnap: cannot resolve config directory, using defaults");
            return Config::default();
        };
        match Self::load_from_migrating(&path) {
            Ok((config, migrated)) => {
                if migrated {
                    // Persist the shifted combinations once, so the next
                    // launch does not migrate again.
                    if let Err(e) = config.save_to(&path) {
                        eprintln!("minisnap: cannot persist migrated config: {e}");
                    }
                }
                config
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let config = Config::default();
                if let Err(e) = config.save_to(&path) {
                    eprintln!("minisnap: cannot write default config: {e}");
                }
                config
            }
            Err(e) => {
                eprintln!(
                    "minisnap: cannot read config {}: {e}; using defaults",
                    path.display()
                );
                Config::default()
            }
        }
    }
}

/// On-disk shape of a config: the editor hotkey is optional so pre-editor
/// files load and go through the migration.
#[derive(Debug, Deserialize)]
struct RawConfig {
    capture_hotkey: HotKey,
    capture_edit_hotkey: Option<HotKey>,
    open_hotkey: HotKey,
    save_dir: PathBuf,
    retention: u32,
}

/// One-time migration of configs written before the editor existed: a
/// capture combination still on the old default ⌘⌥5 shifts to ⌘⌥4 and the
/// editor takes ⌘⌥5; custom combinations stay and the editor still gets
/// its default ⌘⌥5. Returns whether anything changed (i.e. the file should
/// be re-persisted).
fn migrate_legacy_defaults(config: &mut Config, had_edit_key: bool) -> bool {
    if had_edit_key {
        return false;
    }
    if config.capture_hotkey == legacy_capture_hotkey() {
        config.capture_hotkey = default_capture_hotkey();
    }
    config.capture_edit_hotkey = default_capture_edit_hotkey();
    true
}

impl Default for Config {
    fn default() -> Self {
        let pictures = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Config::default_with(pictures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minisnap-config-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_match_spec() {
        let cfg = Config::default_with(PathBuf::from("/tmp/pictures"));
        assert_eq!(
            cfg.capture_hotkey,
            HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit4)
        );
        assert_eq!(
            cfg.capture_edit_hotkey,
            HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit5)
        );
        assert_eq!(
            cfg.open_hotkey,
            HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit0)
        );
        assert_eq!(cfg.save_dir, PathBuf::from("/tmp/pictures/minisnap"));
        assert_eq!(cfg.retention, 100);
    }

    #[test]
    fn hotkey_strings_parse_into_defaults() {
        let capture: HotKey = "alt+meta+Digit4".parse().unwrap();
        assert_eq!(capture, default_capture_hotkey());
        let edit: HotKey = "alt+meta+Digit5".parse().unwrap();
        assert_eq!(edit, default_capture_edit_hotkey());
        let open: HotKey = "alt+meta+Digit0".parse().unwrap();
        assert_eq!(open, default_open_hotkey());
    }

    #[test]
    fn config_round_trips_through_toml() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join("config.toml");
        let cfg = Config::default_with(PathBuf::from("/tmp/pictures"));
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn legacy_default_capture_shifts_to_the_editor() {
        let dir = temp_dir("migrate-legacy");
        let path = dir.join("config.toml");
        // A pre-editor config: capture on the old ⌘⌥5 default, no edit key.
        std::fs::write(
            &path,
            "capture_hotkey = \"alt+meta+Digit5\"\n\
             open_hotkey = \"alt+meta+Digit0\"\n\
             save_dir = \"/tmp/pictures/minisnap\"\n\
             retention = 100\n",
        )
        .unwrap();
        let (cfg, migrated) = Config::load_from_migrating(&path).unwrap();
        assert!(migrated);
        assert_eq!(cfg.capture_hotkey, default_capture_hotkey(), "capture → ⌘⌥4");
        assert_eq!(
            cfg.capture_edit_hotkey,
            default_capture_edit_hotkey(),
            "editor takes ⌘⌥5"
        );

        // Persisting the migrated config and reloading is stable (no second
        // migration).
        cfg.save_to(&path).unwrap();
        let (cfg2, migrated2) = Config::load_from_migrating(&path).unwrap();
        assert!(!migrated2);
        assert_eq!(cfg2, cfg);
    }

    #[test]
    fn custom_combinations_survive_the_migration() {
        let dir = temp_dir("migrate-custom");
        let path = dir.join("config.toml");
        // A pre-editor config with a custom capture combination: it stays.
        std::fs::write(
            &path,
            "capture_hotkey = \"alt+meta+Digit7\"\n\
             open_hotkey = \"alt+meta+Digit0\"\n\
             save_dir = \"/tmp/pictures/minisnap\"\n\
             retention = 100\n",
        )
        .unwrap();
        let (cfg, migrated) = Config::load_from_migrating(&path).unwrap();
        assert_eq!(
            cfg.capture_hotkey,
            HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::Digit7)
        );
        assert_eq!(
            cfg.capture_edit_hotkey,
            default_capture_edit_hotkey(),
            "editor gets its default ⌘⌥5"
        );
        assert!(migrated, "the edit key was absent and got filled in");

        // A modern config is never touched.
        let modern = dir.join("modern.toml");
        Config::default_with(PathBuf::from("/tmp/pictures"))
            .save_to(&modern)
            .unwrap();
        let (cfg, migrated) = Config::load_from_migrating(&modern).unwrap();
        assert!(!migrated);
        assert_eq!(cfg, Config::default_with(PathBuf::from("/tmp/pictures")));
    }
}
