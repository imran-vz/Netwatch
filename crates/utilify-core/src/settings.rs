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
    /// Master switch for the [`crate::alerts`] notifications.
    pub notify_enabled: bool,
    /// Fans forced high while on battery.
    pub notify_fans_battery: bool,
    /// Battery low during a keep-awake session.
    pub notify_low_battery_awake: bool,
    /// Switched to battery while a session blocks sleep on lid close.
    pub notify_lid_battery: bool,
    /// A timed session is about to end.
    pub notify_awake_ending: bool,
    /// Serious or critical thermal pressure.
    pub notify_thermal: bool,
    /// The overheating guard put the fans back on automatic.
    pub notify_fan_overheat: bool,
    /// The fan helper went away while fans were manual.
    pub notify_fan_helper: bool,
    /// Another app took over the fans.
    pub notify_foreign_fans: bool,
    /// The network has been offline for a minute.
    pub notify_offline: bool,
    /// A speed test finished (the app posts this one itself).
    pub notify_speed_test: bool,
    /// "Low battery" for [`Settings::notify_low_battery_awake`], in percent.
    pub low_battery_pct: u8,
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
            notify_enabled: true,
            notify_fans_battery: true,
            notify_low_battery_awake: true,
            notify_lid_battery: true,
            notify_awake_ending: false,
            notify_thermal: true,
            notify_fan_overheat: true,
            notify_fan_helper: true,
            notify_foreign_fans: true,
            notify_offline: false,
            notify_speed_test: true,
            low_battery_pct: 20,
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

    /// Which [`crate::alerts`] notifications to post. Storage uses
    /// [`Settings::storage_notify`]; the low-battery threshold is kept in
    /// 5..=50 if the file was edited by hand.
    pub fn alert_prefs(&self) -> crate::alerts::AlertPrefs {
        let low = self.low_battery_pct.clamp(5, 50);
        crate::alerts::AlertPrefs {
            enabled: self.notify_enabled,
            fans_high_on_battery: self.notify_fans_battery,
            low_battery_awake: self.notify_low_battery_awake,
            lid_sleep_on_battery: self.notify_lid_battery,
            awake_ending_soon: self.notify_awake_ending,
            thermal_pressure: self.notify_thermal,
            fan_overheat: self.notify_fan_overheat,
            fan_helper_lost: self.notify_fan_helper,
            foreign_fan_control: self.notify_foreign_fans,
            network_offline: self.notify_offline,
            storage_red: self.storage_notify,
            speed_test: self.notify_speed_test,
            low_battery_pct: low,
            critical_battery_pct: (low / 2).min(10),
        }
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

/// Load settings; a missing or unreadable file gives the defaults. One that
/// doesn't parse is moved aside first ([`crate::quarantine`]).
pub fn load(path: &Path) -> Settings {
    crate::load_json(path, "settings").unwrap_or_default()
}

/// Save atomically.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    crate::write_atomic(path, &serde_json::to_vec_pretty(settings)?)
}

#[cfg(test)]
pub(crate) mod tests {
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
            notify_enabled: false,
            notify_fans_battery: false,
            notify_low_battery_awake: false,
            notify_lid_battery: false,
            notify_awake_ending: true,
            notify_thermal: false,
            notify_fan_overheat: false,
            notify_fan_helper: false,
            notify_foreign_fans: false,
            notify_offline: true,
            notify_speed_test: false,
            low_battery_pct: 30,
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
        assert!(!path.exists(), "moved aside, not left to be overwritten");
        let aside = only_corrupt_copy(path.parent().unwrap());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "not json");
        assert!(crate::recovered_contains(&aside));
    }

    /// The one `*.corrupt-*` file in `dir`.
    pub(crate) fn only_corrupt_copy(dir: &Path) -> PathBuf {
        let found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        found.into_iter().next().unwrap()
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
    fn alert_prefs_follow_the_settings() {
        use crate::alerts::{AlertKind, AlertPrefs};
        let s = Settings::default();
        assert_eq!(s.alert_prefs(), AlertPrefs::default());
        let off = Settings {
            storage_notify: false,
            notify_offline: true,
            low_battery_pct: 90,
            ..Default::default()
        };
        let p = off.alert_prefs();
        assert!(!p.allows(AlertKind::StorageRed));
        assert!(p.allows(AlertKind::NetworkOffline));
        assert_eq!((p.low_battery_pct, p.critical_battery_pct), (50, 10));
        let muted = Settings {
            notify_enabled: false,
            ..Default::default()
        };
        assert!(
            AlertKind::ALL
                .iter()
                .all(|&k| !muted.alert_prefs().allows(k))
        );
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
