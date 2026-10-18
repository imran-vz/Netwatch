//! Persisted preferences: `<data dir>/dev.netwatch.app/settings.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Refresh intervals offered in the menu, in milliseconds.
pub const INTERVALS_MS: [u64; 4] = [500, 1000, 2000, 5000];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// How often the counters are sampled and the icon redrawn.
    pub interval_ms: u64,
    /// Show the download/upload rates as text next to the icon.
    pub show_rates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            show_rates: true,
        }
    }
}

impl Settings {
    /// The sampling interval, clamped to something sane if the file was
    /// edited by hand.
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms.clamp(250, 60_000))
    }
}

pub fn default_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join(crate::APP_IDENTIFIER)
            .join("settings.json"),
    )
}

/// Load settings; a missing or unreadable file gives the defaults.
pub fn load(path: &Path) -> Settings {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!("Ignoring unreadable settings at {}: {e}", path.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

/// Save atomically (temp file + rename).
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        assert_eq!(load(&path), Settings::default());
        let s = Settings {
            interval_ms: 2000,
            show_rates: false,
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        std::fs::write(&path, r#"{"interval_ms": 500}"#).unwrap();
        assert_eq!(
            load(&path),
            Settings {
                interval_ms: 500,
                show_rates: true
            }
        );
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn interval_is_clamped() {
        let s = Settings {
            interval_ms: 1,
            ..Default::default()
        };
        assert_eq!(s.interval(), Duration::from_millis(250));
    }
}
