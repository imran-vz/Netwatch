//! Human-readable sizes and rates. Decimal units (1 KB = 1000 B), like
//! Finder and Activity Monitor.

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// Scale a value to the largest unit that keeps it >= 1, with a precision
/// that keeps about three significant digits.
fn scaled(value: f64, units: &[&'static str]) -> (String, &'static str) {
    let mut v = value.max(0.0);
    let mut i = 0;
    while v >= 999.5 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    let digits = if i == 0 || v >= 99.95 {
        0
    } else if v >= 9.995 {
        1
    } else {
        2
    };
    (format!("{v:.digits$}"), units[i])
}

/// `1.23 GB`
pub fn bytes(n: u64) -> String {
    let (v, u) = scaled(n as f64, &UNITS);
    format!("{v} {u}")
}

/// `12.3 MB/s`
pub fn rate(bytes_per_sec: f64) -> String {
    let (v, u) = scaled(bytes_per_sec, &UNITS);
    format!("{v} {u}/s")
}

/// Fixed-width (4 character) rate for the menu bar, so the status item
/// doesn't jitter: `  0K`, ` 12K`, `1.2M`, `120M`.
pub fn rate_compact(bytes_per_sec: f64) -> String {
    let units = ["K", "M", "G", "T"];
    let mut v = bytes_per_sec.max(0.0) / 1000.0;
    let mut i = 0;
    while v >= 999.5 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    let num = if v < 9.95 && i > 0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    };
    format!("{num:>3}{}", units[i])
}

/// Link speed: `1 Gbps`, `866 Mbps`, `2.5 Gbps`.
pub fn bits_per_sec(bps: u64) -> String {
    let units = ["bps", "Kbps", "Mbps", "Gbps", "Tbps"];
    let mut v = bps as f64;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    let s = if v >= 10.0 || (v - v.round()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    format!("{s} {}", units[i])
}

/// `42%`, or `<1%` for small but non-zero shares.
pub fn percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    if p > 0.0 && p < 1.0 {
        "<1%".into()
    } else {
        format!("{:.0}%", p.round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_234), "1.23 KB");
        assert_eq!(bytes(12_345_678), "12.3 MB");
        assert_eq!(bytes(494_384_795_648), "494 GB");
        assert_eq!(bytes(999_999), "1.00 MB");
    }

    #[test]
    fn formats_rates() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(1_500_000.0), "1.50 MB/s");
        assert_eq!(rate(-5.0), "0 B/s");
    }

    #[test]
    fn compact_rates_have_a_fixed_width() {
        for (v, s) in [
            (0.0, "  0K"),
            (12_000.0, " 12K"),
            (1_200_000.0, "1.2M"),
            (120_000_000.0, "120M"),
            (2_500_000_000.0, "2.5G"),
        ] {
            assert_eq!(rate_compact(v), s);
            assert_eq!(rate_compact(v).chars().count(), 4);
        }
    }

    #[test]
    fn formats_link_speeds() {
        assert_eq!(bits_per_sec(1_000_000_000), "1 Gbps");
        assert_eq!(bits_per_sec(866_700_000), "867 Mbps");
        assert_eq!(bits_per_sec(2_500_000_000), "2.5 Gbps");
    }

    #[test]
    fn formats_percent() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.426), "43%");
    }
}
