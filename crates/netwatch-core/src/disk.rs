//! Capacity and free space of the startup volume.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    pub available: u64,
}

impl DiskUsage {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Occupied share of the volume, 0..=1.
    pub fn used_fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

/// The startup volume. On APFS, `/` is the sealed system snapshot but it
/// shares its container with the data volume, so both report the same space.
pub fn read_root() -> Option<DiskUsage> {
    read(Path::new("/"))
}

/// Usage of the volume holding `path`.
///
/// macOS: Finder's numbers ("important usage" counts purgeable space, such as
/// iCloud caches and local snapshots, as available), falling back to
/// `statvfs`.
pub fn read(path: &Path) -> Option<DiskUsage> {
    #[cfg(target_os = "macos")]
    if let Some(u) = finder_usage(path) {
        return Some(u);
    }
    statvfs_usage(path)
}

fn statvfs_usage(path: &Path) -> Option<DiskUsage> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    Some(DiskUsage {
        total: st.f_blocks as u64 * frsize,
        available: st.f_bavail as u64 * frsize,
    })
}

#[cfg(target_os = "macos")]
fn finder_usage(path: &Path) -> Option<DiskUsage> {
    use objc2_foundation::{
        NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
        NSURLVolumeTotalCapacityKey,
    };
    let path = NSString::from_str(path.to_str()?);
    let url = NSURL::fileURLWithPath(&path);
    // SAFETY: the key statics are provided by Foundation.
    let (total_key, avail_key) = unsafe {
        (
            NSURLVolumeTotalCapacityKey,
            NSURLVolumeAvailableCapacityForImportantUsageKey,
        )
    };
    let keys = NSArray::from_slice(&[total_key, avail_key]);
    let values = url.resourceValuesForKeys_error(&keys).ok()?;
    let number = |key| -> Option<u64> {
        let n = values.objectForKey(key)?.downcast::<NSNumber>().ok()?;
        u64::try_from(n.longLongValue()).ok()
    };
    let usage = DiskUsage {
        total: number(total_key)?,
        available: number(avail_key)?,
    };
    (usage.total > 0).then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_fraction_is_clamped() {
        let u = DiskUsage {
            total: 400,
            available: 100,
        };
        assert_eq!(u.used(), 300);
        assert_eq!(u.used_fraction(), 0.75);
        let empty = DiskUsage {
            total: 0,
            available: 0,
        };
        assert_eq!(empty.used_fraction(), 0.0);
        let odd = DiskUsage {
            total: 10,
            available: 20,
        };
        assert_eq!(odd.used(), 0);
    }

    #[test]
    fn reads_the_root_volume() {
        let u = read_root().expect("root volume");
        assert!(u.total > 0 && u.available <= u.total, "{u:?}");
        let s = statvfs_usage(Path::new("/")).unwrap();
        assert!(s.total > 0);
    }
}
