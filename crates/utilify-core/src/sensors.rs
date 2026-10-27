//! Named temperature sensors, read without root, for the dashboard's sensor
//! list and for sensor-based fan curves.
//!
//! Apple Silicon only (anything else gets an empty list). Two sources:
//!
//! - **HID**: the temperature services of the IOHIDEventSystem private API
//!   (usage page 0xff00, usage 5). Their `Product` strings are readable:
//!   "pACC MTR Temp Sensor3", "PMU tdie4", "gas gauge battery", ...
//! - **SMC**: `T*` keys of the System Management Controller. Their names are
//!   cryptic and model-specific, so only keys with a well-known meaning are
//!   named one by one; the CPU and GPU families are summarized instead.
//!
//! # Naming rules
//!
//! HID products:
//!
//! | product                       | name                       | group   |
//! |-------------------------------|----------------------------|---------|
//! | `pACC MTR Temp Sensor<n>`     | CPU Performance Core *k*   | CPU     |
//! | `eACC MTR Temp Sensor<n>`     | CPU Efficiency Core *k*    | CPU     |
//! | `GPU MTR Temp Sensor<n>`      | GPU Cluster *k*            | GPU     |
//! | `SOC MTR Temp Sensor<n>`      | SoC *k*                    | SoC     |
//! | `PMGR SOC Die Temp Sensor<n>` | Power Manager Die *k*      | SoC     |
//! | `ANE MTR Temp Sensor<n>`      | Neural Engine              | SoC     |
//! | `ISP MTR Temp Sensor<n>`      | Image Signal Processor     | SoC     |
//! | `PMU<p> tdie<n>`              | SoC Die *n* [(PMU *p*)]    | SoC     |
//! | `PMU<p> tdev<n>`              | SoC Thermistor *n* [(PMU *p*)] | SoC |
//! | `PMU<p> tcal`                 | dropped (calibration constant) |     |
//! | `gas gauge battery`           | Battery Gas Gauge          | Battery |
//! | `NAND CH<n> temp`             | SSD (NAND) [channel *n*]   | Storage |
//! | contains airport/wifi/wlan    | Wi-Fi                      | Wireless|
//! | `als-temp`                    | Ambient Light Sensor       | Ambient |
//! | anything else                 | the product string         | Other   |
//!
//! *k* is the rank of the raw index *n* among the sensors of that kind (raw
//! indices are sparse), so the names count from 1; ids keep the raw index so
//! they stay stable. Services with the same product (here every PMU sensor
//! appears three times, and the gas gauge six) are merged into one sensor
//! showing the hottest of them: the conservative value for a fan curve.
//! Readings outside -20..=150 °C are dropped (the PMU `tdev` thermistors
//! read about -9200 °C on an M4 Pro).
//!
//! SMC keys (only `flt ` values, zeros dropped as "not populated"):
//!
//! - `Tp**`: CPU Performance Cores (average) / (hottest).
//! - `Te**`: CPU Efficiency Cores (average) / (hottest).
//! - `Tg**`: GPU (average) / (hottest).
//!   Averages skip keys whose third character is `x` (`Tpx0`, `Tex1`:
//!   per-cluster maxima, which would double count).
//! - `TB0T` Battery, `TB1T`/`TB2T` Battery 1/2, `TH0x` SSD, `TW0P` Wi-Fi
//!   (Airport Proximity), `Ts0P`/`Ts1P` Palm Rest 1/2, `TA0P` Ambient.
//!
//! When HID and SMC name the same thing (the SSD), HID wins. Finally a
//! "CPU Core Average" (`cpu_core_average`) is added: the mean of the HID
//! per-core sensors when there are any (M1/M2), else of the SMC `Tp`/`Te`
//! keys (M3/M4, which publish no per-core HID sensors).

use std::collections::BTreeMap;

/// What a sensor measures, for grouping in the UI. Ordered as displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SensorGroup {
    Cpu,
    Gpu,
    Soc,
    Battery,
    Storage,
    Wireless,
    Ambient,
    Enclosure,
    Other,
}

impl SensorGroup {
    pub fn label(self) -> &'static str {
        match self {
            SensorGroup::Cpu => "CPU",
            SensorGroup::Gpu => "GPU",
            SensorGroup::Soc => "SoC",
            SensorGroup::Battery => "Battery",
            SensorGroup::Storage => "Storage",
            SensorGroup::Wireless => "Wireless",
            SensorGroup::Ambient => "Ambient",
            SensorGroup::Enclosure => "Enclosure",
            SensorGroup::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    /// Stable key, for settings (e.g. the sensor a fan curve follows).
    pub id: String,
    pub name: String,
    pub group: SensorGroup,
    pub celsius: f32,
}

/// The id of the synthesized "CPU Core Average" sensor.
pub const CPU_AVERAGE_ID: &str = "cpu_core_average";

/// All sensors, grouped and sorted for display. Empty when unsupported or
/// when reading fails. Reuses one [`SensorReader`] across calls.
///
/// Blocks for about 50 ms on an M4 Pro (about 3 ms of it CPU time; the rest
/// is waiting on one IPC per HID service and SMC key), so call it off the UI
/// thread. The first call also opens the reader (about 100 ms).
pub fn read_all() -> Vec<Sensor> {
    static READER: std::sync::Mutex<Option<SensorReader>> = std::sync::Mutex::new(None);
    let mut guard = READER.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(SensorReader::new).read()
}

/// The CPU core average: the synthesized sensor, else the mean of the CPU
/// group.
pub fn cpu_average(sensors: &[Sensor]) -> Option<f32> {
    if let Some(s) = sensors.iter().find(|s| s.id == CPU_AVERAGE_ID) {
        return Some(s.celsius);
    }
    mean(
        sensors
            .iter()
            .filter(|s| s.group == SensorGroup::Cpu)
            .map(|s| s.celsius),
    )
}

/// The hottest sensor.
pub fn hottest(sensors: &[Sensor]) -> Option<&Sensor> {
    sensors
        .iter()
        .max_by(|a, b| a.celsius.total_cmp(&b.celsius))
}

/// Looks up a sensor by [`Sensor::id`].
pub fn find<'a>(sensors: &'a [Sensor], id: &str) -> Option<&'a Sensor> {
    sensors.iter().find(|s| s.id == id)
}

/// Holds the HID client, its service list and the SMC connection with the
/// keys worth reading, so each [`SensorReader::read`] only reads values.
pub struct SensorReader {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    hid: Option<hid::Client>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    smc: Option<smc::Reader>,
}

impl Default for SensorReader {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorReader {
    pub fn new() -> Self {
        Self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            hid: hid::Client::open(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            smc: smc::Reader::open(),
        }
    }

    pub fn read(&mut self) -> Vec<Sensor> {
        build(&self.read_raw())
    }

    /// Unnamed readings, as the hardware reports them.
    pub fn read_raw(&mut self) -> Vec<Raw> {
        #[allow(unused_mut)]
        let mut raw = Vec::new();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            if let Some(hid) = &self.hid {
                raw.extend(hid.read().into_iter().map(|(p, c)| Raw::Hid(p, c)));
            }
            if let Some(smc) = &mut self.smc {
                raw.extend(smc.read().into_iter().map(|(k, c)| Raw::Smc(k, c)));
            }
        }
        raw
    }
}

/// One reading before naming: a HID product string or an SMC key.
#[derive(Clone, Debug, PartialEq)]
pub enum Raw {
    Hid(String, f32),
    Smc([u8; 4], f32),
}

fn plausible(c: f32) -> bool {
    (-20.0..=150.0).contains(&c)
}

fn mean(values: impl IntoIterator<Item = f32>) -> Option<f32> {
    let (sum, n) = values
        .into_iter()
        .fold((0.0f64, 0u32), |(s, n), v| (s + v as f64, n + 1));
    (n > 0).then(|| (sum / n as f64) as f32)
}

/// What a HID product string denotes. Ordered for display within a group.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum HidKind {
    PCore(u32),
    ECore(u32),
    Gpu(u32),
    Soc(u32),
    PmgrDie(u32),
    Ane,
    Isp,
    PmuDie(u32, u32),
    PmuDev(u32, u32),
    Battery,
    Nand(u32),
    Wifi,
    Ambient,
    Other(String),
}

/// The trailing decimal number of `s` and the text before it.
fn split_number(s: &str) -> Option<(&str, u32)> {
    let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let (head, n) = s.split_at(s.len() - digits);
    Some((head, n.parse().ok()?))
}

/// `None` for products that aren't temperatures worth showing.
fn classify(product: &str) -> Option<HidKind> {
    let p = product.trim();
    let mtr = |prefix: &str| -> Option<u32> {
        let rest = p.strip_prefix(prefix)?.trim();
        Some(split_number(rest).map_or(0, |(_, n)| n))
    };
    if let Some(n) = mtr("pACC MTR Temp Sensor") {
        return Some(HidKind::PCore(n));
    }
    if let Some(n) = mtr("eACC MTR Temp Sensor") {
        return Some(HidKind::ECore(n));
    }
    if let Some(n) = mtr("GPU MTR Temp Sensor") {
        return Some(HidKind::Gpu(n));
    }
    if let Some(n) = mtr("SOC MTR Temp Sensor") {
        return Some(HidKind::Soc(n));
    }
    if let Some(n) = mtr("PMGR SOC Die Temp Sensor") {
        return Some(HidKind::PmgrDie(n));
    }
    if mtr("ANE MTR Temp Sensor").is_some() {
        return Some(HidKind::Ane);
    }
    if mtr("ISP MTR Temp Sensor").is_some() {
        return Some(HidKind::Isp);
    }
    if let Some(rest) = p.strip_prefix("PMU")
        && let Some((unit, sensor)) = rest.split_once(' ')
        && (unit.is_empty() || unit.bytes().all(|b| b.is_ascii_digit()))
    {
        let pmu = unit.parse().unwrap_or(1);
        return match split_number(sensor) {
            Some(("tdie", n)) => Some(HidKind::PmuDie(pmu, n)),
            Some(("tdev", n)) => Some(HidKind::PmuDev(pmu, n)),
            _ if sensor == "tcal" => None,
            _ => Some(HidKind::Other(p.to_string())),
        };
    }
    if p.eq_ignore_ascii_case("gas gauge battery") {
        return Some(HidKind::Battery);
    }
    if let Some(rest) = p.strip_prefix("NAND CH")
        && let Some((n, _)) = rest.split_once(' ')
        && let Ok(n) = n.parse()
    {
        return Some(HidKind::Nand(n));
    }
    let lower = p.to_ascii_lowercase();
    if ["airport", "wifi", "wi-fi", "wlan"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return Some(HidKind::Wifi);
    }
    if lower == "als-temp" {
        return Some(HidKind::Ambient);
    }
    Some(HidKind::Other(p.to_string()))
}

fn pmu_suffix(pmu: u32, multi: bool) -> String {
    if multi {
        format!(" (PMU {pmu})")
    } else {
        String::new()
    }
}

fn hid_sensors(hid: &BTreeMap<HidKind, f32>) -> Vec<Sensor> {
    let multi_pmu = hid
        .keys()
        .filter_map(|k| match k {
            HidKind::PmuDie(p, _) | HidKind::PmuDev(p, _) => Some(*p),
            _ => None,
        })
        .any(|p| p != 1);
    // Rank within each numbered family, in raw-index order (the map is sorted).
    let mut ranks: std::collections::HashMap<std::mem::Discriminant<HidKind>, u32> =
        Default::default();
    let mut out = Vec::new();
    for (kind, &celsius) in hid {
        let rank = ranks
            .entry(std::mem::discriminant(kind))
            .and_modify(|r| *r += 1)
            .or_insert(1);
        let k = *rank;
        let (id, name, group) = match kind {
            HidKind::PCore(n) => (
                format!("cpu_p_{n}"),
                format!("CPU Performance Core {k}"),
                SensorGroup::Cpu,
            ),
            HidKind::ECore(n) => (
                format!("cpu_e_{n}"),
                format!("CPU Efficiency Core {k}"),
                SensorGroup::Cpu,
            ),
            HidKind::Gpu(n) => (
                format!("gpu_{n}"),
                format!("GPU Cluster {k}"),
                SensorGroup::Gpu,
            ),
            HidKind::Soc(n) => (format!("soc_{n}"), format!("SoC {k}"), SensorGroup::Soc),
            HidKind::PmgrDie(n) => (
                format!("pmgr_die_{n}"),
                format!("Power Manager Die {k}"),
                SensorGroup::Soc,
            ),
            HidKind::Ane => ("ane".into(), "Neural Engine".into(), SensorGroup::Soc),
            HidKind::Isp => (
                "isp".into(),
                "Image Signal Processor".into(),
                SensorGroup::Soc,
            ),
            HidKind::PmuDie(p, n) => (
                format!("pmu{p}_tdie{n}"),
                format!("SoC Die {n}{}", pmu_suffix(*p, multi_pmu)),
                SensorGroup::Soc,
            ),
            HidKind::PmuDev(p, n) => (
                format!("pmu{p}_tdev{n}"),
                format!("SoC Thermistor {n}{}", pmu_suffix(*p, multi_pmu)),
                SensorGroup::Soc,
            ),
            HidKind::Battery => (
                "battery_gas_gauge".into(),
                "Battery Gas Gauge".into(),
                SensorGroup::Battery,
            ),
            HidKind::Nand(0) => ("ssd".into(), "SSD (NAND)".into(), SensorGroup::Storage),
            HidKind::Nand(n) => (
                format!("ssd_ch{n}"),
                format!("SSD (NAND channel {n})"),
                SensorGroup::Storage,
            ),
            HidKind::Wifi => ("wifi".into(), "Wi-Fi".into(), SensorGroup::Wireless),
            HidKind::Ambient => (
                "ambient_light".into(),
                "Ambient Light Sensor".into(),
                SensorGroup::Ambient,
            ),
            HidKind::Other(p) => (format!("hid:{p}"), p.clone(), SensorGroup::Other),
        };
        out.push(Sensor {
            id,
            name,
            group,
            celsius,
        });
    }
    out
}

/// SMC keys with a well-known meaning.
const SMC_NAMED: &[(&[u8; 4], &str, &str, SensorGroup)] = &[
    (b"TB0T", "battery", "Battery", SensorGroup::Battery),
    (b"TB1T", "battery_1", "Battery 1", SensorGroup::Battery),
    (b"TB2T", "battery_2", "Battery 2", SensorGroup::Battery),
    (b"TH0x", "ssd", "SSD", SensorGroup::Storage),
    (
        b"TW0P",
        "wifi",
        "Wi-Fi (Airport Proximity)",
        SensorGroup::Wireless,
    ),
    (b"TA0P", "ambient", "Ambient", SensorGroup::Ambient),
    (
        b"Ts0P",
        "palm_rest_1",
        "Palm Rest 1",
        SensorGroup::Enclosure,
    ),
    (
        b"Ts1P",
        "palm_rest_2",
        "Palm Rest 2",
        SensorGroup::Enclosure,
    ),
];

/// Summarized SMC families: key prefix, id stem, name stem, group.
const SMC_FAMILIES: &[(&[u8; 2], &str, &str, SensorGroup)] = &[
    (b"Tp", "cpu_p", "CPU Performance Cores", SensorGroup::Cpu),
    (b"Te", "cpu_e", "CPU Efficiency Cores", SensorGroup::Cpu),
    (b"Tg", "gpu", "GPU", SensorGroup::Gpu),
];

/// Whether the SMC reader should keep `key` at all.
fn smc_wanted(key: &[u8; 4]) -> bool {
    SMC_NAMED.iter().any(|(k, ..)| *k == key)
        || SMC_FAMILIES.iter().any(|(p, ..)| key.starts_with(*p))
}

/// Per-cluster aggregates (`Tpx0`), left out of averages.
fn smc_is_aggregate(key: &[u8; 4]) -> bool {
    key[2] == b'x'
}

fn smc_sensors(smc: &[([u8; 4], f32)]) -> Vec<Sensor> {
    let mut out = Vec::new();
    for (prefix, id, name, group) in SMC_FAMILIES {
        let family = || smc.iter().filter(|(k, _)| k.starts_with(*prefix));
        let avg = mean(
            family()
                .filter(|(k, _)| !smc_is_aggregate(k))
                .map(|(_, c)| *c),
        );
        let max = family().map(|(_, c)| *c).max_by(f32::total_cmp);
        if let (Some(avg), Some(max)) = (avg, max) {
            out.push(Sensor {
                id: format!("{id}_avg"),
                name: format!("{name} (average)"),
                group: *group,
                celsius: avg,
            });
            out.push(Sensor {
                id: format!("{id}_max"),
                name: format!("{name} (hottest)"),
                group: *group,
                celsius: max,
            });
        }
    }
    for (key, id, name, group) in SMC_NAMED {
        if let Some((_, c)) = smc.iter().find(|(k, _)| k == *key) {
            out.push(Sensor {
                id: (*id).into(),
                name: (*name).into(),
                group: *group,
                celsius: *c,
            });
        }
    }
    out
}

/// Names, merges and sorts raw readings (see the module docs for the rules).
pub fn build(raw: &[Raw]) -> Vec<Sensor> {
    let mut hid: BTreeMap<HidKind, f32> = BTreeMap::new();
    let mut smc: Vec<([u8; 4], f32)> = Vec::new();
    for r in raw {
        match r {
            Raw::Hid(product, c) if plausible(*c) => {
                if let Some(kind) = classify(product) {
                    let v = hid.entry(kind).or_insert(*c);
                    *v = v.max(*c);
                }
            }
            Raw::Smc(key, c)
                if plausible(*c)
                    && *c != 0.0
                    && smc_wanted(key)
                    && !smc.iter().any(|(k, _)| k == key) =>
            {
                smc.push((*key, *c));
            }
            _ => {}
        }
    }

    let hid_cores = mean(
        hid.iter()
            .filter(|(k, _)| matches!(k, HidKind::PCore(_) | HidKind::ECore(_)))
            .map(|(_, c)| *c),
    );
    let smc_cores = mean(
        smc.iter()
            .filter(|(k, _)| (k.starts_with(b"Tp") || k.starts_with(b"Te")) && !smc_is_aggregate(k))
            .map(|(_, c)| *c),
    );

    let mut out = Vec::new();
    if let Some(c) = hid_cores.or(smc_cores) {
        out.push(Sensor {
            id: CPU_AVERAGE_ID.into(),
            name: "CPU Core Average".into(),
            group: SensorGroup::Cpu,
            celsius: c,
        });
    }
    for s in hid_sensors(&hid).into_iter().chain(smc_sensors(&smc)) {
        if !out.iter().any(|o| o.id == s.id) {
            out.push(s);
        }
    }
    // Stable: keeps the order above within each group.
    out.sort_by_key(|s| s.group);
    out
}

/// IOHIDEventSystemClient: private, but what Activity Monitor-style tools
/// use to read SoC temperatures without root.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod hid {
    use std::ffi::{c_char, c_void};

    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOHID_EVENT_TYPE_TEMPERATURE: i64 = 15;
    const TEMPERATURE_LEVEL_FIELD: i32 = 15 << 16;

    #[repr(C)]
    struct CFDictionaryCallBacks([usize; 6]);

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryCallBacks;
        fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
        fn CFStringCreateWithCString(
            alloc: CFTypeRef,
            s: *const c_char,
            encoding: u32,
        ) -> CFTypeRef;
        fn CFDictionaryCreate(
            alloc: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: CFIndex,
            key_callbacks: *const CFDictionaryCallBacks,
            value_callbacks: *const CFDictionaryCallBacks,
        ) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: CFIndex, enc: u32) -> u8;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDEventSystemClientCreate(alloc: CFTypeRef) -> CFTypeRef;
        fn IOHIDEventSystemClientSetMatching(client: CFTypeRef, matching: CFTypeRef) -> i32;
        fn IOHIDEventSystemClientCopyServices(client: CFTypeRef) -> CFTypeRef;
        fn IOHIDServiceClientCopyProperty(service: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn IOHIDServiceClientCopyEvent(
            service: CFTypeRef,
            kind: i64,
            options: i32,
            timestamp: i64,
        ) -> CFTypeRef;
        fn IOHIDEventGetFloatValue(event: CFTypeRef, field: i32) -> f64;
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

    fn cfnum(n: i32) -> Option<Owned> {
        // SAFETY: `n` is a live i32, matching kCFNumberSInt32Type.
        Owned::new(unsafe {
            CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_SINT32_TYPE,
                (&n as *const i32).cast(),
            )
        })
    }

    /// The client and its temperature services, with their product names.
    pub struct Client {
        _client: Owned,
        services: Owned,
        /// (index into `services`, product) of the services worth polling.
        live: Vec<(CFIndex, String)>,
    }

    // SAFETY: the CF objects are only used through `&self` from one thread at
    // a time (the reader is behind a Mutex or owned by one thread); CF and
    // IOHIDEventSystemClient objects aren't tied to the creating thread.
    unsafe impl Send for Client {}

    impl Client {
        /// Each service read is an IPC round trip (about 1 ms), so services
        /// that are dropped by name or gave no plausible reading at open
        /// (here: `PMU tcal`, `als-temp` and the `PMU tdev` thermistors,
        /// 28 of 77) are never polled again.
        pub fn open() -> Option<Self> {
            // SAFETY: plain CF/IOKit calls; every returned object is checked
            // for null and owned by an `Owned`.
            let mut client = unsafe {
                let client = Owned::new(IOHIDEventSystemClientCreate(std::ptr::null()))?;
                let keys = [cfstr(c"PrimaryUsagePage")?, cfstr(c"PrimaryUsage")?];
                let values = [cfnum(0xff00)?, cfnum(5)?];
                let keys = keys.each_ref().map(|k| k.0);
                let values = values.each_ref().map(|v| v.0);
                let matching = Owned::new(CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    2,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                ))?;
                IOHIDEventSystemClientSetMatching(client.0, matching.0);
                let services = Owned::new(IOHIDEventSystemClientCopyServices(client.0))?;
                let product_key = cfstr(c"Product")?;
                let live = (0..CFArrayGetCount(services.0))
                    .filter_map(|i| {
                        let service = CFArrayGetValueAtIndex(services.0, i);
                        let p = Owned::new(IOHIDServiceClientCopyProperty(service, product_key.0))?;
                        let product = to_string(p.0)?;
                        super::classify(&product).is_some().then_some((i, product))
                    })
                    .collect();
                Self {
                    _client: client,
                    services,
                    live,
                }
            };
            let live = std::mem::take(&mut client.live);
            client.live = live
                .into_iter()
                .filter(|(i, _)| client.read_one(*i).is_some_and(super::plausible))
                .collect();
            Some(client)
        }

        fn read_one(&self, i: CFIndex) -> Option<f32> {
            // SAFETY: `i` came from the array, which `self` keeps alive; the
            // event is owned and released by `Owned`.
            let celsius = unsafe {
                let service = CFArrayGetValueAtIndex(self.services.0, i);
                let event = Owned::new(IOHIDServiceClientCopyEvent(
                    service,
                    K_IOHID_EVENT_TYPE_TEMPERATURE,
                    0,
                    0,
                ))?;
                IOHIDEventGetFloatValue(event.0, TEMPERATURE_LEVEL_FIELD)
            };
            celsius.is_finite().then_some(celsius as f32)
        }

        /// (product, °C) per polled service that currently has a reading.
        pub fn read(&self) -> Vec<(String, f32)> {
            self.live
                .iter()
                .filter_map(|(i, p)| Some((p.clone(), self.read_one(*i)?)))
                .collect()
        }
    }

    /// SAFETY: `cf` must be a live CF object.
    unsafe fn to_string(cf: CFTypeRef) -> Option<String> {
        // SAFETY: per the contract; the buffer length is passed along.
        unsafe {
            if CFGetTypeID(cf) != CFStringGetTypeID() {
                return None;
            }
            let mut buf = [0 as c_char; 256];
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
}

/// The SMC side: `utilify-smc`'s client (reading needs no root), with the
/// keys worth reading found once at open, so each read is one call per key.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod smc {
    use utilify_smc::{DataType, Key, KeyInfo, Smc, SmcIo};

    pub struct Reader {
        smc: Smc,
        /// `flt ` keys worth reading, found once at open.
        pub(super) keys: Vec<(Key, KeyInfo)>,
    }

    impl Reader {
        pub fn open() -> Option<Self> {
            let smc = match Smc::open() {
                Ok(smc) => smc,
                Err(e) => {
                    log::debug!("Opening AppleSMC failed ({e})");
                    return None;
                }
            };
            let keys = smc
                .keys()
                .unwrap_or_default()
                .into_iter()
                .filter(|k| super::smc_wanted(&k.bytes()))
                .filter_map(|k| {
                    let info = smc.key_info(k).ok()?;
                    (info.data_type == DataType::FLT && info.size == 4).then_some((k, info))
                })
                .collect();
            Some(Self { smc, keys })
        }

        pub fn read(&mut self) -> Vec<([u8; 4], f32)> {
            self.keys
                .iter()
                .filter_map(|&(k, info)| {
                    // Non-finite values don't decode.
                    let c = self.smc.read_known(k, info).ok()?.as_f64()?;
                    Some((k.bytes(), c as f32))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HID products and values recorded on a Mac16,7 (M4 Pro), macOS 27.
    fn m4_pro_hid() -> Vec<Raw> {
        // "als-temp" is listed too but returns no temperature event.
        let mut raw = Vec::new();
        for v in [32.0, 32.6, 33.0, 33.6, 33.6, 33.6] {
            raw.push(Raw::Hid("gas gauge battery".into(), v));
        }
        raw.push(Raw::Hid("NAND CH0 temp".into(), 32.0));
        for _ in 0..3 {
            raw.push(Raw::Hid("PMU tcal".into(), 51.82));
            for n in 1..=8 {
                raw.push(Raw::Hid(format!("PMU tdev{n}"), -9199.43));
            }
        }
        for (n, vals) in [
            (1, [62.52, 62.68, 62.76]),
            (2, [53.48, 53.56, 54.04]),
            (3, [55.56, 55.64, 55.72]),
            (14, [52.84, 52.92, 52.92]),
        ] {
            for v in vals {
                raw.push(Raw::Hid(format!("PMU tdie{n}"), v));
            }
        }
        raw
    }

    /// A subset of the SMC `T*` keys recorded on the same machine.
    fn m4_pro_smc() -> Vec<Raw> {
        [
            ("TB0T", 33.5),
            ("TB1T", 32.5),
            ("TB2T", 33.5),
            ("TH0x", 32.39),
            ("TW0P", 49.04),
            ("Ts0P", 32.62),
            ("Ts1P", 30.06),
            ("Ta00", 0.0),
            ("TPD0", 66.99),
            ("Tp01", 77.21),
            ("Tp05", 82.86),
            ("Tpx5", 90.98),
            ("Te05", 67.71),
            ("Te0S", 62.87),
            ("Tex1", 71.11),
            ("Tg0K", 51.29),
            ("Tg1k", 52.53),
        ]
        .into_iter()
        .map(|(k, c)| Raw::Smc(k.as_bytes().try_into().unwrap(), c))
        .collect()
    }

    fn names(sensors: &[Sensor]) -> Vec<(&str, &str, SensorGroup)> {
        sensors
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str(), s.group))
            .collect()
    }

    #[test]
    fn m4_pro_hid_is_merged_and_filtered() {
        let s = build(&m4_pro_hid());
        assert_eq!(
            names(&s),
            [
                ("pmu1_tdie1", "SoC Die 1", SensorGroup::Soc),
                ("pmu1_tdie2", "SoC Die 2", SensorGroup::Soc),
                ("pmu1_tdie3", "SoC Die 3", SensorGroup::Soc),
                ("pmu1_tdie14", "SoC Die 14", SensorGroup::Soc),
                (
                    "battery_gas_gauge",
                    "Battery Gas Gauge",
                    SensorGroup::Battery
                ),
                ("ssd", "SSD (NAND)", SensorGroup::Storage),
            ]
        );
        // Duplicates show the hottest copy.
        assert_eq!(find(&s, "pmu1_tdie1").unwrap().celsius, 62.76);
        assert_eq!(find(&s, "battery_gas_gauge").unwrap().celsius, 33.6);
        // No per-core sensors and no SMC: no core average.
        assert_eq!(cpu_average(&s), None);
    }

    #[test]
    fn m4_pro_with_smc() {
        let mut raw = m4_pro_hid();
        raw.extend(m4_pro_smc());
        let s = build(&raw);
        assert_eq!(
            names(&s)
                .into_iter()
                .filter(|(_, _, g)| *g != SensorGroup::Soc)
                .collect::<Vec<_>>(),
            [
                ("cpu_core_average", "CPU Core Average", SensorGroup::Cpu),
                (
                    "cpu_p_avg",
                    "CPU Performance Cores (average)",
                    SensorGroup::Cpu
                ),
                (
                    "cpu_p_max",
                    "CPU Performance Cores (hottest)",
                    SensorGroup::Cpu
                ),
                (
                    "cpu_e_avg",
                    "CPU Efficiency Cores (average)",
                    SensorGroup::Cpu
                ),
                (
                    "cpu_e_max",
                    "CPU Efficiency Cores (hottest)",
                    SensorGroup::Cpu
                ),
                ("gpu_avg", "GPU (average)", SensorGroup::Gpu),
                ("gpu_max", "GPU (hottest)", SensorGroup::Gpu),
                (
                    "battery_gas_gauge",
                    "Battery Gas Gauge",
                    SensorGroup::Battery
                ),
                ("battery", "Battery", SensorGroup::Battery),
                ("battery_1", "Battery 1", SensorGroup::Battery),
                ("battery_2", "Battery 2", SensorGroup::Battery),
                ("ssd", "SSD (NAND)", SensorGroup::Storage),
                ("wifi", "Wi-Fi (Airport Proximity)", SensorGroup::Wireless),
                ("palm_rest_1", "Palm Rest 1", SensorGroup::Enclosure),
                ("palm_rest_2", "Palm Rest 2", SensorGroup::Enclosure),
            ]
        );
        let c = |id| find(&s, id).unwrap().celsius;
        // Aggregates (`x` keys) count for "hottest" but not the averages.
        assert_eq!(c("cpu_p_max"), 90.98);
        assert!((c("cpu_p_avg") - 80.035).abs() < 1e-3);
        assert!((c("cpu_core_average") - (77.21 + 82.86 + 67.71 + 62.87) / 4.0).abs() < 1e-3);
        assert_eq!(cpu_average(&s), Some(c("cpu_core_average")));
        // HID wins over SMC for the same id.
        assert_eq!(c("ssd"), 32.0);
        assert_eq!(hottest(&s).unwrap().id, "cpu_p_max");
    }

    #[test]
    fn m1_style_products_are_ranked() {
        let raw: Vec<Raw> = [
            ("pACC MTR Temp Sensor9", 50.0),
            ("pACC MTR Temp Sensor2", 48.0),
            ("pACC MTR Temp Sensor5", 46.0),
            ("eACC MTR Temp Sensor0", 40.0),
            ("eACC MTR Temp Sensor3", 42.0),
            ("GPU MTR Temp Sensor4", 38.0),
            ("GPU MTR Temp Sensor1", 39.0),
            ("SOC MTR Temp Sensor0", 41.0),
            ("ANE MTR Temp Sensor1", 35.0),
            ("ISP MTR Temp Sensor5", 36.0),
            ("PMGR SOC Die Temp Sensor1", 44.0),
            ("PMU tdev1", 30.0),
            ("PMU2 tdie3", 45.0),
            ("NAND CH1 temp", 33.0),
            ("Airport temp", 37.0),
            ("Mystery Sensor", 31.0),
        ]
        .into_iter()
        .map(|(p, c)| Raw::Hid(p.into(), c))
        .collect();
        let s = build(&raw);
        assert_eq!(
            names(&s),
            [
                ("cpu_core_average", "CPU Core Average", SensorGroup::Cpu),
                ("cpu_p_2", "CPU Performance Core 1", SensorGroup::Cpu),
                ("cpu_p_5", "CPU Performance Core 2", SensorGroup::Cpu),
                ("cpu_p_9", "CPU Performance Core 3", SensorGroup::Cpu),
                ("cpu_e_0", "CPU Efficiency Core 1", SensorGroup::Cpu),
                ("cpu_e_3", "CPU Efficiency Core 2", SensorGroup::Cpu),
                ("gpu_1", "GPU Cluster 1", SensorGroup::Gpu),
                ("gpu_4", "GPU Cluster 2", SensorGroup::Gpu),
                ("soc_0", "SoC 1", SensorGroup::Soc),
                ("pmgr_die_1", "Power Manager Die 1", SensorGroup::Soc),
                ("ane", "Neural Engine", SensorGroup::Soc),
                ("isp", "Image Signal Processor", SensorGroup::Soc),
                ("pmu2_tdie3", "SoC Die 3 (PMU 2)", SensorGroup::Soc),
                ("pmu1_tdev1", "SoC Thermistor 1 (PMU 1)", SensorGroup::Soc),
                ("ssd_ch1", "SSD (NAND channel 1)", SensorGroup::Storage),
                ("wifi", "Wi-Fi", SensorGroup::Wireless),
                ("hid:Mystery Sensor", "Mystery Sensor", SensorGroup::Other),
            ]
        );
        assert_eq!(cpu_average(&s), Some(45.2));
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(build(&[]).is_empty());
        assert_eq!(cpu_average(&[]), None);
        assert!(hottest(&[]).is_none());
    }

    /// Prints every raw reading and the named sensors, and times reads:
    /// `cargo test -p utilify-core sensors::tests::dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump() {
        let t = std::time::Instant::now();
        let mut reader = SensorReader::new();
        println!("open: {:?}", t.elapsed());
        let mut raw = reader.read_raw();
        raw.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        for r in &raw {
            match r {
                Raw::Hid(p, c) => println!("hid {p:?} {c:.2}"),
                Raw::Smc(k, c) => println!("smc {} {c:.2}", String::from_utf8_lossy(k)),
            }
        }
        for s in reader.read() {
            println!(
                "{:<10} {:<22} {:<36} {:.1}",
                s.group.label(),
                s.id,
                s.name,
                s.celsius
            );
        }
        let cpu = || {
            let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
            // SAFETY: valid out-pointer.
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
            std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        };
        let n = 20;
        let (t, c) = (std::time::Instant::now(), cpu());
        for _ in 0..n {
            std::hint::black_box(reader.read());
        }
        println!(
            "read: {:?} per call, {:?} of it on the CPU",
            t.elapsed() / n,
            (cpu() - c) / n
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(reader.hid.as_ref().map(|h| h.read()));
            }
            println!("  hid: {:?} per call", t.elapsed() / n);
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(reader.smc.as_mut().map(|s| s.read()));
            }
            let keys = reader.smc.as_ref().map_or(0, |s| s.keys.len());
            println!("  smc: {:?} per call ({keys} keys)", t.elapsed() / n);
        }
    }
}
