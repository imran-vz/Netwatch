//! Persisted preferences: `<data dir>/dev.utilify.app/settings.json`.

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
    /// Show a temperature next to the icon...
    pub show_temp: bool,
    /// ...from this sensor (a [`crate::sensors::Sensor::id`]).
    pub menu_temp_sensor: String,
    /// Show the fastest fan's speed next to the icon.
    pub show_fan_rpm: bool,
    /// The storage bar turns yellow from this much used, in percent.
    pub storage_warn_pct: u8,
    /// ...and red from this much.
    pub storage_danger_pct: u8,
    /// Post a notification when storage crosses into red.
    pub storage_notify: bool,
    /// Keep-awake sessions let the display sleep (only the system stays up).
    pub awake_allow_display_sleep: bool,
    /// Keep-awake sessions let the Mac sleep when the lid closes. Blocking
    /// it only works on AC power.
    pub awake_allow_closed_display_sleep: bool,
    /// While display sleep is blocked, start the screen saver after
    /// [`Settings::awake_screen_saver_mins`] of inactivity.
    pub awake_screen_saver: bool,
    pub awake_screen_saver_mins: u16,
    /// Start an indefinite session when Utilify launches.
    pub awake_on_launch: bool,
    /// Post a notification when a session ends by itself.
    pub awake_notify_end: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            show_rates: true,
            show_temp: false,
            menu_temp_sensor: crate::sensors::CPU_AVERAGE_ID.into(),
            show_fan_rpm: false,
            storage_warn_pct: 50,
            storage_danger_pct: 80,
            storage_notify: true,
            awake_allow_display_sleep: false,
            awake_allow_closed_display_sleep: true,
            awake_screen_saver: false,
            awake_screen_saver_mins: 45,
            awake_on_launch: false,
            awake_notify_end: true,
        }
    }
}

impl Settings {
    /// The sampling interval, clamped to something sane if the file was
    /// edited by hand.
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms.clamp(250, 60_000))
    }

    /// The storage thresholds as fractions (yellow, red), kept in order and
    /// in range if the file was edited by hand.
    pub fn storage_thresholds(&self) -> (f64, f64) {
        let danger = self.storage_danger_pct.clamp(2, 100);
        let warn = self.storage_warn_pct.clamp(1, danger - 1);
        (warn as f64 / 100.0, danger as f64 / 100.0)
    }

    /// What keep-awake sessions let the Mac do.
    pub fn awake_allow(&self) -> crate::awake::Allow {
        crate::awake::Allow {
            display_sleep: self.awake_allow_display_sleep,
            closed_display_sleep: self.awake_allow_closed_display_sleep,
        }
    }

    /// The screen saver delay, kept sane if the file was edited by hand.
    pub fn awake_screen_saver_after(&self) -> Duration {
        Duration::from_secs(self.awake_screen_saver_mins.clamp(1, 24 * 60) as u64 * 60)
    }

    /// Which colour the storage bar shows at `used` (0..=1).
    pub fn storage_level(&self, used: f64) -> StorageLevel {
        let (warn, danger) = self.storage_thresholds();
        if used >= danger {
            StorageLevel::Danger
        } else if used >= warn {
            StorageLevel::Warn
        } else {
            StorageLevel::Ok
        }
    }
}

/// Storage bar colour zones: green, yellow, red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageLevel {
    Ok,
    Warn,
    Danger,
}

/// Fires once each time storage crosses into red. It re-arms only after
/// usage drops [`StorageAlarm::REARM_BELOW`] under the red threshold, so a
/// disk hovering at the line doesn't notify over and over. The first
/// reading never fires: an app that starts already in the red shows it in
/// the bar instead of notifying at every login.
#[derive(Clone, Debug, Default)]
pub struct StorageAlarm {
    armed: Option<bool>,
}

impl StorageAlarm {
    pub const REARM_BELOW: f64 = 0.02;

    /// Feed a reading; true when this one should notify.
    pub fn update(&mut self, used: f64, danger: f64) -> bool {
        let red = used >= danger;
        match self.armed {
            None => {
                self.armed = Some(!red);
                false
            }
            Some(true) if red => {
                self.armed = Some(false);
                true
            }
            Some(false) if used < danger - Self::REARM_BELOW => {
                self.armed = Some(true);
                false
            }
            _ => false,
        }
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

/// Save atomically.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    crate::write_atomic(path, &serde_json::to_vec_pretty(settings)?)
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
            show_temp: true,
            menu_temp_sensor: "gpu_max".into(),
            show_fan_rpm: true,
            storage_warn_pct: 60,
            storage_danger_pct: 90,
            storage_notify: false,
            awake_allow_display_sleep: true,
            awake_allow_closed_display_sleep: false,
            awake_screen_saver: true,
            awake_screen_saver_mins: 10,
            awake_on_launch: true,
            awake_notify_end: false,
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        std::fs::write(&path, r#"{"interval_ms": 500}"#).unwrap();
        assert_eq!(
            load(&path),
            Settings {
                interval_ms: 500,
                ..Default::default()
            }
        );
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn storage_levels_follow_the_thresholds() {
        let s = Settings::default();
        assert_eq!(s.storage_level(0.49), StorageLevel::Ok);
        assert_eq!(s.storage_level(0.50), StorageLevel::Warn);
        assert_eq!(s.storage_level(0.79), StorageLevel::Warn);
        assert_eq!(s.storage_level(0.80), StorageLevel::Danger);
        // Out of order by hand: yellow stays below red.
        let odd = Settings {
            storage_warn_pct: 95,
            storage_danger_pct: 70,
            ..Default::default()
        };
        assert_eq!(odd.storage_thresholds(), (0.69, 0.70));
    }

    #[test]
    fn alarm_fires_once_per_crossing() {
        let mut a = StorageAlarm::default();
        assert!(!a.update(0.70, 0.80), "first reading never fires");
        assert!(a.update(0.81, 0.80));
        assert!(!a.update(0.82, 0.80), "still red: no repeat");
        assert!(!a.update(0.79, 0.80), "inside the re-arm margin");
        assert!(!a.update(0.80, 0.80));
        assert!(!a.update(0.77, 0.80), "re-armed");
        assert!(a.update(0.80, 0.80));

        let mut started_red = StorageAlarm::default();
        assert!(!started_red.update(0.90, 0.80));
        assert!(!started_red.update(0.91, 0.80));
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
