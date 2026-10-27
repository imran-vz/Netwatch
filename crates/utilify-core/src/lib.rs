//! Utilify backend. No UI and no gpui dependency.
//!
//! - [`awake`]: keep-awake sessions (power assertions), like Amphetamine.
//! - [`net`]: per-interface byte counters and link speed, and the
//!   [`net::NetMeter`] that turns two readings into rates.
//! - [`disk`]: capacity and free space of the startup volume.
//! - [`disk_history`]: storage use over time, persisted across launches.
//! - [`fans`]: fan presets and curves, and control through the root helper.
//! - [`monitor`]: one tick of sampling, plus the rolling history.
//! - [`icon`]: the menu-bar graph, built as SVG and rasterized with resvg.
//! - [`format`]: human-readable sizes and rates.
//! - [`sensors`]: named temperature sensors (Apple Silicon).
//! - [`settings`]: the persisted preferences.
//! - [`speedtest`]: on-demand speed test via `networkQuality`.

pub mod awake;
pub mod disk;
pub mod disk_history;
pub mod fans;
pub mod format;
pub mod history;
pub mod icon;
pub mod monitor;
pub mod net;
pub mod sensors;
pub mod settings;
pub mod speedtest;

/// Names the per-user data directory.
pub const APP_IDENTIFIER: &str = "dev.utilify.app";
pub const PRODUCT_NAME: &str = "Utilify";
/// The data directory's name before the app was renamed from Netwatch.
pub const LEGACY_APP_IDENTIFIER: &str = "dev.netwatch.app";

/// Write `bytes` to `path` atomically, creating its directory: readers see
/// the old file or the new one, never a partial write. Each save gets its own
/// temporary file, so overlapping saves of one path can't corrupt it.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Move the pre-rename data directory to [`APP_IDENTIFIER`], once: only when
/// the old one exists and the new one doesn't. Call before loading anything.
pub fn migrate_legacy_data() {
    let Some(base) = dirs::data_dir() else { return };
    if let Err(e) = migrate_data_dir(&base) {
        log::warn!("Moving {LEGACY_APP_IDENTIFIER} data failed: {e}");
    }
}

fn migrate_data_dir(base: &std::path::Path) -> std::io::Result<bool> {
    let (old, new) = (base.join(LEGACY_APP_IDENTIFIER), base.join(APP_IDENTIFIER));
    if !old.is_dir() || new.exists() {
        return Ok(false);
    }
    std::fs::rename(&old, &new)?;
    log::info!("Moved {} to {}", old.display(), new.display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_legacy_data_once() {
        let base = tempfile::tempdir().unwrap();
        assert!(!migrate_data_dir(base.path()).unwrap(), "nothing to move");
        let old = base.path().join(LEGACY_APP_IDENTIFIER);
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("settings.json"), "{}").unwrap();
        assert!(migrate_data_dir(base.path()).unwrap());
        assert!(
            base.path()
                .join(APP_IDENTIFIER)
                .join("settings.json")
                .is_file()
        );
        assert!(!old.exists());
        // An existing new directory is never overwritten.
        std::fs::create_dir(&old).unwrap();
        assert!(!migrate_data_dir(base.path()).unwrap());
        assert!(old.exists());
    }

    #[test]
    fn overlapping_atomic_writes_leave_one_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("data.json");
        let writers: Vec<_> = (0..8u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        write_atomic(&path, &vec![b'a' + i; 64 * 1024]).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 64 * 1024);
        assert!(bytes.iter().all(|&b| b == bytes[0]));
        // No temporary files are left behind.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
