//! Storage use over time, kept across launches:
//! `<data dir>/dev.utilify.app/disk_history.json`.

use crate::disk::DiskUsage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Minimum spacing between stored samples.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(15 * 60);

/// Samples older than this are dropped.
pub const RETAIN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSample {
    /// Unix time, seconds.
    pub at: u64,
    pub used: u64,
    pub total: u64,
}

/// Samples, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskHistory {
    samples: Vec<DiskSample>,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl DiskHistory {
    pub fn samples(&self) -> &[DiskSample] {
        &self.samples
    }

    /// Store `usage` if the last sample is at least [`SAMPLE_EVERY`] old (or
    /// the clock went backwards), dropping samples past [`RETAIN`]. Returns
    /// whether anything changed, i.e. whether to save.
    pub fn record(&mut self, now: u64, usage: DiskUsage) -> bool {
        let due = self
            .samples
            .last()
            .is_none_or(|last| now < last.at || now - last.at >= SAMPLE_EVERY.as_secs());
        if !due {
            return false;
        }
        let cutoff = now.saturating_sub(RETAIN.as_secs());
        self.samples.retain(|s| s.at >= cutoff && s.at <= now);
        self.samples.push(DiskSample {
            at: now,
            used: usage.used(),
            total: usage.total,
        });
        true
    }

    /// Stored samples plus a live reading at `now`, for plotting up to the
    /// present.
    pub fn with_live(&self, now: u64, live: Option<DiskUsage>) -> Vec<DiskSample> {
        let mut v = self.samples.clone();
        if let Some(u) = live
            && v.last().is_none_or(|l| l.at < now)
        {
            v.push(DiskSample {
                at: now,
                used: u.used(),
                total: u.total,
            });
        }
        v
    }
}

pub fn default_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join(crate::APP_IDENTIFIER)
            .join("disk_history.json"),
    )
}

/// Load the history; a missing or unreadable file gives an empty one.
pub fn load(path: &Path) -> DiskHistory {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!(
                "Ignoring unreadable disk history at {}: {e}",
                path.display()
            );
            DiskHistory::default()
        }),
        Err(_) => DiskHistory::default(),
    }
}

/// Save atomically.
pub fn save(path: &Path, history: &DiskHistory) -> std::io::Result<()> {
    crate::write_atomic(path, &serde_json::to_vec(history)?)
}

/// Polyline points for `samples` (oldest first) within a `width` x `height`
/// box: x by time, y by used bytes, scaled to the visible range (centered,
/// with some headroom) so small changes stay visible. Empty when there is nothing to
/// draw.
pub fn graph_points(samples: &[DiskSample], width: f32, height: f32) -> Vec<(f32, f32)> {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return Vec::new();
    };
    let span = (last.at.saturating_sub(first.at)).max(1) as f64;
    let lo = samples.iter().map(|s| s.used).min().unwrap_or(0) as f64;
    let hi = samples.iter().map(|s| s.used).max().unwrap_or(0) as f64;
    // At least 1% of the volume, so noise doesn't fill the whole height.
    let total = last.total as f64;
    let range = (hi - lo).max(total * 0.01).max(1.0);
    // Center the data vertically, with some headroom.
    let range = range * 1.3;
    let lo = (lo + hi) / 2.0 - range / 2.0;
    samples
        .iter()
        .map(|s| {
            let x = (s.at.saturating_sub(first.at)) as f64 / span * width as f64;
            let y = height as f64 * (1.0 - (s.used as f64 - lo) / range);
            (x as f32, y.clamp(0.0, height as f64) as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: u64) -> DiskUsage {
        DiskUsage {
            total: 1_000,
            available: 1_000 - used,
        }
    }

    #[test]
    fn records_at_most_every_interval_and_prunes() {
        let step = SAMPLE_EVERY.as_secs();
        let mut h = DiskHistory::default();
        assert!(h.record(1_000, usage(100)));
        assert!(!h.record(1_000 + step - 1, usage(110)));
        assert!(h.record(1_000 + step, usage(120)));
        assert_eq!(h.samples().len(), 2);
        assert_eq!(h.samples()[1].used, 120);

        let later = 1_000 + RETAIN.as_secs() + step;
        assert!(h.record(later, usage(130)));
        let at: Vec<u64> = h.samples().iter().map(|s| s.at).collect();
        assert_eq!(at, [1_000 + step, later]);

        // Clock moved backwards: drop the samples from the "future".
        assert!(h.record(500 + step, usage(90)));
        assert_eq!(h.samples().len(), 1);
    }

    #[test]
    fn live_reading_is_appended() {
        let mut h = DiskHistory::default();
        assert!(h.with_live(5, None).is_empty());
        h.record(5, usage(100));
        assert_eq!(h.with_live(5, Some(usage(200))).len(), 1);
        let v = h.with_live(9, Some(usage(200)));
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].used, 200);
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/disk_history.json");
        assert_eq!(load(&path), DiskHistory::default());
        let mut h = DiskHistory::default();
        h.record(10, usage(100));
        save(&path, &h).unwrap();
        assert_eq!(load(&path), h);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(load(&path), DiskHistory::default());
    }

    #[test]
    fn graph_spans_the_box() {
        let s = |at, used| DiskSample {
            at,
            used,
            total: 1_000,
        };
        assert!(graph_points(&[], 100.0, 50.0).is_empty());
        let pts = graph_points(&[s(0, 100), s(50, 400), s(100, 200)], 100.0, 50.0);
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].0, 0.0);
        assert_eq!(pts[2].0, 100.0);
        // Higher usage is drawn higher (smaller y), inside the box.
        assert!(pts[1].1 < pts[2].1 && pts[2].1 < pts[0].1);
        assert!(pts.iter().all(|p| (0.0..=50.0).contains(&p.1)));
    }
}
