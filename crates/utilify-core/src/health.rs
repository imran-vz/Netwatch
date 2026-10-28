//! Periodic self-checks: can we write our data, are the tools and readings
//! we rely on there, do the persisted files parse. Each check is a few file
//! reads or syscalls, so [`run_core_checks`] can run off the main thread
//! every 15 minutes. The app adds its own checks (helper, notifications)
//! next to these.

use serde::de::DeserializeOwned;
use std::path::Path;

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Ok,
    /// Works, but something is off or missing.
    Warn,
    /// Broken: some feature won't work.
    Fail,
}

/// One check's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Stable id, e.g. `data-dir`.
    pub id: &'static str,
    /// What was checked, for the list: `Data folder`.
    pub name: String,
    pub status: Health,
    /// What was found: `Writable`, `Not created yet`, the error.
    pub detail: String,
    /// An action the app can offer to fix it (one of the `FIX_*` ids).
    pub fix: Option<&'static str>,
}

/// Fix action: reveal the data folder in Finder.
pub const FIX_REVEAL_DATA_DIR: &str = "reveal-data-dir";

impl Check {
    fn new(id: &'static str, name: &str, status: Health, detail: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            status,
            detail: detail.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: &'static str) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// The worst status among `checks` ([`Health::Ok`] when empty).
pub fn overall(checks: &[Check]) -> Health {
    checks.iter().map(|c| c.status).max().unwrap_or(Health::Ok)
}

/// Every core check, against the real data directory.
pub fn run_core_checks() -> Vec<Check> {
    match dirs::data_dir() {
        Some(base) => run_core_checks_in(&base.join(crate::APP_IDENTIFIER)),
        None => vec![Check::new(
            "data-dir",
            "Data folder",
            Health::Fail,
            "No Application Support folder",
        )],
    }
}

/// Every core check, with the data files in `data_dir`.
pub fn run_core_checks_in(data_dir: &Path) -> Vec<Check> {
    vec![
        check_data_dir(data_dir),
        check_speed_test(),
        check_disk(),
        check_network(),
        check_file::<crate::settings::Settings>(
            "settings-file",
            "Settings file",
            &data_dir.join("settings.json"),
        ),
        check_file::<crate::fans::FanSettings>(
            "fans-file",
            "Fan presets file",
            &data_dir.join("fans.json"),
        ),
        check_file::<crate::awake::Stored>(
            "awake-file",
            "Keep-awake total file",
            &data_dir.join("awake.json"),
        ),
        check_file::<crate::disk_history::DiskHistory>(
            "disk-history-file",
            "Storage history file",
            &data_dir.join("disk_history.json"),
        ),
    ]
}

/// The data directory exists (or can be made) and takes a write.
pub fn check_data_dir(dir: &Path) -> Check {
    const NAME: &str = "Data folder";
    let probe = dir.join(format!(".health-probe-{}", std::process::id()));
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe));
    match result {
        Ok(()) => Check::new("data-dir", NAME, Health::Ok, "Writable"),
        Err(e) => {
            log::warn!("{} isn't writable: {e}", dir.display());
            Check::new("data-dir", NAME, Health::Fail, format!("Not writable: {e}"))
                .with_fix(FIX_REVEAL_DATA_DIR)
        }
    }
}

/// `networkQuality` is there for the speed test.
pub fn check_speed_test() -> Check {
    const NAME: &str = "Speed test tool";
    if crate::speedtest::is_available() {
        Check::new("speed-test", NAME, Health::Ok, "networkQuality available")
    } else {
        Check::new(
            "speed-test",
            NAME,
            Health::Warn,
            "networkQuality is missing (needs macOS 12 or later)",
        )
    }
}

/// The startup volume's usage can be read.
pub fn check_disk() -> Check {
    const NAME: &str = "Storage reading";
    match crate::disk::read_root() {
        Some(u) if u.total > 0 => Check::new(
            "disk",
            NAME,
            Health::Ok,
            format!("{:.0}% used", u.used_fraction() * 100.0),
        ),
        _ => Check::new("disk", NAME, Health::Fail, "Couldn't read the startup disk"),
    }
}

/// There is a network interface to count.
pub fn check_network() -> Check {
    match crate::net::read_interfaces() {
        Ok(ifaces) => network_check(&ifaces),
        Err(e) => Check::new(
            "network",
            "Network interfaces",
            Health::Fail,
            format!("Couldn't list them: {e}"),
        ),
    }
}

/// [`check_network`] on a listing: a physical port must exist; being
/// offline isn't a fault.
fn network_check(ifaces: &[crate::net::InterfaceCounters]) -> Check {
    const NAME: &str = "Network interfaces";
    let physical: Vec<_> = ifaces.iter().filter(|i| i.is_physical()).collect();
    if physical.is_empty() {
        return Check::new("network", NAME, Health::Warn, "No Ethernet or Wi-Fi port");
    }
    let active = physical.iter().filter(|i| i.is_active()).count();
    Check::new(
        "network",
        NAME,
        Health::Ok,
        format!("{} found, {active} active", physical.len()),
    )
}

/// A persisted file parses. Missing is fine (not saved yet). Reads only:
/// moving a bad file aside is the loader's job.
fn check_file<T: DeserializeOwned>(id: &'static str, name: &str, path: &Path) -> Check {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Check::new(id, name, Health::Ok, "Not created yet")
        }
        Err(e) => Check::new(id, name, Health::Fail, format!("Unreadable: {e}"))
            .with_fix(FIX_REVEAL_DATA_DIR),
        Ok(bytes) => match serde_json::from_slice::<T>(&bytes) {
            Ok(_) => Check::new(id, name, Health::Ok, "OK"),
            Err(e) => Check::new(id, name, Health::Warn, format!("Doesn't parse: {e}"))
                .with_fix(FIX_REVEAL_DATA_DIR),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_id<'a>(checks: &'a [Check], id: &str) -> &'a Check {
        checks.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn fresh_dir_is_healthy() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("app");
        let checks = run_core_checks_in(&dir);
        assert_eq!(by_id(&checks, "data-dir").status, Health::Ok);
        assert!(dir.is_dir(), "created");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "probe removed");
        for id in [
            "settings-file",
            "fans-file",
            "awake-file",
            "disk-history-file",
        ] {
            let c = by_id(&checks, id);
            assert_eq!(c.status, Health::Ok, "{c:?}");
            assert_eq!(c.detail, "Not created yet");
        }
        // Ids are unique.
        let mut ids: Vec<_> = checks.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), checks.len());
    }

    #[test]
    fn spots_files_that_dont_parse() {
        let dir = tempfile::tempdir().unwrap();
        crate::settings::save(
            &dir.path().join("settings.json"),
            &crate::settings::Settings::default(),
        )
        .unwrap();
        crate::awake::save_total(&dir.path().join("awake.json"), Default::default()).unwrap();
        std::fs::write(dir.path().join("fans.json"), "{nope").unwrap();
        let checks = run_core_checks_in(dir.path());
        assert_eq!(by_id(&checks, "settings-file").status, Health::Ok);
        assert_eq!(by_id(&checks, "awake-file").status, Health::Ok);
        let fans = by_id(&checks, "fans-file");
        assert_eq!(fans.status, Health::Warn);
        assert_eq!(fans.fix, Some(FIX_REVEAL_DATA_DIR));
        assert!(dir.path().join("fans.json").exists(), "left in place");
        assert!(overall(&checks) >= Health::Warn);
    }

    #[test]
    fn unwritable_dir_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the directory should be.
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, "").unwrap();
        let c = check_data_dir(&blocked);
        assert_eq!(c.status, Health::Fail);
        assert_eq!(c.fix, Some(FIX_REVEAL_DATA_DIR));
    }

    #[test]
    fn network_needs_a_physical_port() {
        let iface = |name: &str, up: bool| crate::net::InterfaceCounters {
            name: name.into(),
            up,
            running: up,
            loopback: name == "lo0",
            rx_bytes: 0,
            tx_bytes: 0,
            baudrate: 0,
        };
        assert_eq!(network_check(&[iface("lo0", true)]).status, Health::Warn);
        let c = network_check(&[iface("lo0", true), iface("en0", false), iface("en1", true)]);
        assert_eq!(c.status, Health::Ok);
        assert_eq!(c.detail, "2 found, 1 active");
    }

    #[test]
    fn overall_is_the_worst() {
        assert_eq!(overall(&[]), Health::Ok);
        let c = |s| Check::new("x", "X", s, "");
        assert_eq!(
            overall(&[c(Health::Ok), c(Health::Fail), c(Health::Warn)]),
            Health::Fail
        );
    }

    /// Not `run_core_checks`: that would create the real data folder from a
    /// test run.
    #[test]
    fn real_readings_work() {
        assert_eq!(check_disk().status, Health::Ok);
        assert_ne!(check_network().status, Health::Fail);
    }
}
