//! The power source: on AC or battery, the charge, whether it's charging,
//! and Low Power Mode. Read through IOKit's power-sources API, which asks
//! `powerd` over a mach port: cheap enough to call every few seconds.
//!
//! A desktop Mac (no internal battery) reads as on AC with no battery.

/// What the power source is doing right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerState {
    /// Running from the internal battery, not wall power.
    pub on_battery: bool,
    /// The internal battery's charge, 0..=100.
    pub percent: Option<u8>,
    pub charging: bool,
    /// macOS's estimate; `None` on AC or while it's still working it out.
    pub minutes_to_empty: Option<u32>,
    /// This Mac has an internal battery.
    pub has_battery: bool,
    /// Low Power Mode is on.
    pub low_power_mode: bool,
}

/// One power source's description, as plain values. Only what
/// [`summarize`] needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Source {
    /// `Type`: `InternalBattery`, `UPS`, ...
    pub kind: Option<String>,
    /// `Is Present`; missing counts as present.
    pub present: Option<bool>,
    /// `Power Source State`: `AC Power` or `Battery Power`.
    pub state: Option<String>,
    /// `Current Capacity` and `Max Capacity`: percent on Apple Silicon,
    /// but older Macs report mAh, so the ratio is what counts.
    pub current: Option<i64>,
    pub max: Option<i64>,
    /// `Is Charging`.
    pub charging: Option<bool>,
    /// `Time to Empty`, minutes; -1 while it's being worked out.
    pub time_to_empty: Option<i64>,
}

/// The `IOPSGetProvidingPowerSourceType` value for battery.
const BATTERY_POWER: &str = "Battery Power";
/// The `Type` of a laptop's own battery.
const INTERNAL_BATTERY: &str = "InternalBattery";

/// `current` of `max` as a percentage, rounded and kept in 0..=100. `None`
/// for a missing or zero maximum.
pub(crate) fn percent(current: Option<i64>, max: Option<i64>) -> Option<u8> {
    let (c, m) = (current?, max?);
    if m <= 0 || c < 0 {
        return None;
    }
    Some(((c as f64 / m as f64) * 100.0).round().clamp(0.0, 100.0) as u8)
}

/// Combine the providing source type and each source's description into a
/// [`PowerState`]. The internal battery gives the charge; the providing
/// type (or, failing that, the battery's own state) says whether we're on
/// battery.
pub(crate) fn summarize(
    providing: Option<&str>,
    sources: &[Source],
    low_power_mode: bool,
) -> PowerState {
    let battery = sources
        .iter()
        .find(|s| s.kind.as_deref() == Some(INTERNAL_BATTERY) && s.present.unwrap_or(true));
    let on_battery = match providing {
        Some(p) => p == BATTERY_POWER,
        None => battery.is_some_and(|b| b.state.as_deref() == Some(BATTERY_POWER)),
    };
    let Some(b) = battery else {
        return PowerState {
            on_battery,
            low_power_mode,
            ..Default::default()
        };
    };
    PowerState {
        on_battery,
        percent: percent(b.current, b.max),
        charging: b.charging.unwrap_or(false),
        minutes_to_empty: b
            .time_to_empty
            .filter(|&m| on_battery && m >= 0)
            .and_then(|m| u32::try_from(m).ok()),
        has_battery: true,
        low_power_mode,
    }
}

/// The power source now. Never fails: anything unreadable reads as a
/// desktop on AC. Callers that poll should prefer [`try_read`] and keep
/// the last good reading, so a failed read isn't taken for plugging in.
pub fn read() -> PowerState {
    try_read().unwrap_or_default()
}

/// The power source now, or `None` when macOS didn't answer.
#[cfg(target_os = "macos")]
pub fn try_read() -> Option<PowerState> {
    let Some((providing, sources)) = sys::sources() else {
        log::warn!("Reading the power sources failed");
        return None;
    };
    Some(summarize(providing.as_deref(), &sources, low_power_mode()))
}

#[cfg(not(target_os = "macos"))]
pub fn try_read() -> Option<PowerState> {
    Some(PowerState::default())
}

/// Whether Low Power Mode is on.
#[cfg(target_os = "macos")]
fn low_power_mode() -> bool {
    objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled()
}

#[cfg(target_os = "macos")]
mod sys {
    use super::Source;
    use std::ffi::{c_char, c_void};

    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: CFTypeRef,
            s: *const c_char,
            encoding: u32,
        ) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: CFIndex, enc: u32) -> u8;
        fn CFStringGetTypeID() -> usize;
        fn CFNumberGetValue(n: CFTypeRef, kind: CFIndex, out: *mut c_void) -> u8;
        fn CFNumberGetTypeID() -> usize;
        fn CFBooleanGetValue(b: CFTypeRef) -> u8;
        fn CFBooleanGetTypeID() -> usize;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
        fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFTypeRef;
    }

    /// An owned CF object, released on drop.
    struct Owned(CFTypeRef);

    impl Owned {
        fn new(cf: CFTypeRef) -> Option<Self> {
            (!cf.is_null()).then(|| Self(cf))
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: non-null and owned (from a Create/Copy call).
            unsafe { CFRelease(self.0) };
        }
    }

    fn cfstr(s: &std::ffi::CStr) -> Option<Owned> {
        // SAFETY: `s` is NUL-terminated UTF-8.
        Owned::new(unsafe {
            CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        })
    }

    /// A borrowed CFString's contents; `None` for null or another type.
    fn string(cf: CFTypeRef) -> Option<String> {
        // SAFETY: type checked before use; the buffer outlives the call.
        unsafe {
            if cf.is_null() || CFGetTypeID(cf) != CFStringGetTypeID() {
                return None;
            }
            let mut buf = [0 as c_char; 128];
            if CFStringGetCString(
                cf,
                buf.as_mut_ptr(),
                buf.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
            ) == 0
            {
                return None;
            }
            Some(
                std::ffi::CStr::from_ptr(buf.as_ptr())
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    /// A borrowed dictionary's value for `key`, not owned.
    fn get(dict: CFTypeRef, key: &std::ffi::CStr) -> CFTypeRef {
        let Some(k) = cfstr(key) else {
            return std::ptr::null();
        };
        // SAFETY: `dict` is a live CFDictionary and `k` a live CFString.
        unsafe { CFDictionaryGetValue(dict, k.0) }
    }

    fn int(dict: CFTypeRef, key: &std::ffi::CStr) -> Option<i64> {
        let v = get(dict, key);
        let mut out = 0i64;
        // SAFETY: type checked; `out` matches kCFNumberSInt64Type.
        unsafe {
            (!v.is_null()
                && CFGetTypeID(v) == CFNumberGetTypeID()
                && CFNumberGetValue(v, K_CF_NUMBER_SINT64_TYPE, (&mut out as *mut i64).cast()) != 0)
                .then_some(out)
        }
    }

    fn boolean(dict: CFTypeRef, key: &std::ffi::CStr) -> Option<bool> {
        let v = get(dict, key);
        // SAFETY: type checked before use.
        unsafe {
            (!v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID())
                .then(|| CFBooleanGetValue(v) != 0)
        }
    }

    /// The providing source type and every source's description. `None`
    /// if IOKit gave nothing at all.
    pub fn sources() -> Option<(Option<String>, Vec<Source>)> {
        // SAFETY: a plain query; the blob is released by `Owned`.
        let blob = Owned::new(unsafe { IOPSCopyPowerSourcesInfo() })?;
        // SAFETY: `blob` is live; the result is borrowed from it ("Get").
        let providing = string(unsafe { IOPSGetProvidingPowerSourceType(blob.0) });
        // SAFETY: `blob` is live; the list is owned ("Copy").
        let Some(list) = Owned::new(unsafe { IOPSCopyPowerSourcesList(blob.0) }) else {
            // A desktop can have no list at all.
            return Some((providing, Vec::new()));
        };
        // SAFETY: `list` is a live CFArray.
        let count = unsafe { CFArrayGetCount(list.0) };
        let mut out = Vec::new();
        for i in 0..count {
            // SAFETY: `i` is in range; both results are borrowed.
            let desc =
                unsafe { IOPSGetPowerSourceDescription(blob.0, CFArrayGetValueAtIndex(list.0, i)) };
            if desc.is_null() {
                continue;
            }
            out.push(Source {
                kind: string(get(desc, c"Type")),
                present: boolean(desc, c"Is Present"),
                state: string(get(desc, c"Power Source State")),
                current: int(desc, c"Current Capacity"),
                max: int(desc, c"Max Capacity"),
                charging: boolean(desc, c"Is Charging"),
                time_to_empty: int(desc, c"Time to Empty"),
            });
        }
        Some((providing, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(state: &str, current: i64, charging: bool, tte: i64) -> Source {
        Source {
            kind: Some(INTERNAL_BATTERY.into()),
            present: Some(true),
            state: Some(state.into()),
            current: Some(current),
            max: Some(100),
            charging: Some(charging),
            time_to_empty: Some(tte),
        }
    }

    #[test]
    fn percent_is_a_ratio() {
        assert_eq!(percent(Some(63), Some(100)), Some(63));
        // mAh on older Macs.
        assert_eq!(percent(Some(2500), Some(5000)), Some(50));
        assert_eq!(percent(Some(120), Some(100)), Some(100));
        assert_eq!(percent(Some(5), Some(0)), None);
        assert_eq!(percent(None, Some(100)), None);
        assert_eq!(percent(Some(-1), Some(100)), None);
    }

    #[test]
    fn laptop_on_battery() {
        let s = summarize(
            Some("Battery Power"),
            &[battery("Battery Power", 63, false, 214)],
            true,
        );
        assert_eq!(
            s,
            PowerState {
                on_battery: true,
                percent: Some(63),
                charging: false,
                minutes_to_empty: Some(214),
                has_battery: true,
                low_power_mode: true,
            }
        );
        // Still estimating.
        let s = summarize(
            Some("Battery Power"),
            &[battery("Battery Power", 63, false, -1)],
            false,
        );
        assert_eq!(s.minutes_to_empty, None);
    }

    #[test]
    fn laptop_charging_on_ac() {
        let s = summarize(Some("AC Power"), &[battery("AC Power", 80, true, 0)], false);
        assert!(!s.on_battery && s.charging && s.has_battery);
        assert_eq!(s.minutes_to_empty, None, "no estimate on AC");
        // No providing type: the battery's own state decides.
        assert!(summarize(None, &[battery("Battery Power", 80, false, 9)], false).on_battery);
    }

    #[test]
    fn desktop_has_no_battery() {
        let ups = Source {
            kind: Some("UPS".into()),
            ..Default::default()
        };
        assert_eq!(
            summarize(Some("AC Power"), &[ups], false),
            PowerState::default()
        );
        assert_eq!(summarize(None, &[], false), PowerState::default());
        // A removed battery doesn't count.
        let gone = Source {
            present: Some(false),
            ..battery("AC Power", 0, false, 0)
        };
        assert!(!summarize(Some("AC Power"), &[gone], false).has_battery);
    }

    #[test]
    fn reads_something_sane() {
        let s = read();
        assert!(s.percent.is_none_or(|p| p <= 100));
        if !s.has_battery {
            assert_eq!(s.percent, None);
            assert!(!s.charging);
        }
        if !s.on_battery {
            assert_eq!(s.minutes_to_empty, None);
        }
    }
}
