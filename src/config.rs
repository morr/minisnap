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
    /// Global hotkey that opens the latest saved shot.
    pub open_hotkey: HotKey,
    /// Directory where shots are saved silently.
    pub save_dir: PathBuf,
    /// Maximum number of saved shots kept on disk.
    pub retention: u32,
}

pub fn default_capture_hotkey() -> HotKey {
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
        let text = fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    /// is not overwritten so nothing is lost).
    pub fn load_or_create() -> Config {
        let Some(path) = Self::standard_path() else {
            eprintln!("minisnap: cannot resolve config directory, using defaults");
            return Config::default();
        };
        match Self::load_from(&path) {
            Ok(config) => config,
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
        let capture: HotKey = "alt+meta+Digit5".parse().unwrap();
        assert_eq!(capture, default_capture_hotkey());
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
}
