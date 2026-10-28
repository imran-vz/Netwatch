//! Keep the Mac awake, like Amphetamine. A session holds IOKit power
//! assertions that block sleep: indefinitely, for a while, until a time of
//! day, while an app runs, while a file downloads, or while the network is
//! busy. The time spent awake adds up across launches in
//! `<data dir>/dev.utilify.app/awake.json`.
//!
//! macOS drops a process's assertions when it exits, so a crash can't leave
//! the Mac stuck awake.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// "Minutes" session lengths offered in the menu.
pub const MINUTES: [u64; 5] = [5, 10, 15, 30, 45];
/// "Hours" session lengths offered in the menu.
pub const HOURS: [u64; 8] = [1, 2, 3, 4, 5, 6, 8, 12];

/// "While the network is busy" counts the link as busy from this download
/// rate...
pub const NETWORK_BUSY: f64 = 50_000.0;
/// ...and ends once it has been quiet this long. Counted from the start too,
/// so a session started on an idle link still ends.
pub const NETWORK_GRACE: Duration = Duration::from_secs(2 * 60);
/// "While a file downloads" ends this long after the last in-progress
/// download disappears from the Downloads folder.
pub const FILE_GRACE: Duration = Duration::from_secs(60);

/// File extensions browsers give downloads in progress: Safari (a
/// `.download` bundle), Chrome, Edge, Brave and Arc (`.crdownload`), Firefox
/// (`.part`), Opera (`.opdownload`), and others (`.partial`).
pub const PARTIAL_EXTENSIONS: [&str; 5] =
    ["download", "crdownload", "part", "partial", "opdownload"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Indefinite,
    For(Duration),
    /// Until a local time of day, in minutes after midnight (the next time
    /// it comes round).
    Until(u16),
    /// While the process with this pid runs.
    WhileApp(i32),
    WhileFileDownloading,
    WhileNetworkBusy,
}

impl Mode {
    /// The menu label for starting this mode.
    pub fn label(self) -> String {
        match self {
            Mode::Indefinite => "Indefinitely".into(),
            Mode::For(d) => duration_label(d),
            Mode::Until(m) => format!("Until {}", clock(m)),
            Mode::WhileApp(_) => "While App Is Running".into(),
            Mode::WhileFileDownloading => "While File Is Downloading".into(),
            Mode::WhileNetworkBusy => "While Network Is Busy".into(),
        }
    }
}

/// `5 minutes`, `1 hour`, `1 hour 30 minutes`.
pub fn duration_label(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    let (h, m) = (mins / 60, mins % 60);
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    match (h, m) {
        (0, m) => plural(m, "minute"),
        (h, 0) => plural(h, "hour"),
        (h, m) => format!("{} {}", plural(h, "hour"), plural(m, "minute")),
    }
}

/// `09:05`, from minutes after midnight.
pub fn clock(minute_of_day: u16) -> String {
    format!("{:02}:{:02}", minute_of_day / 60 % 24, minute_of_day % 60)
}

/// How a session started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    Manual,
    AtLaunch,
}

impl Activation {
    pub fn label(self) -> &'static str {
        match self {
            Activation::Manual => "Manual Activation",
            Activation::AtLaunch => "Started at Launch",
        }
    }
}

/// What a tick knows about the outside world. Only the signal the session's
/// mode needs has to be filled in (see [`Mode`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    pub down_rate: f64,
    pub file_downloading: bool,
    pub app_running: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub mode: Mode,
    pub activation: Activation,
    /// The app name, for [`Mode::WhileApp`].
    pub subject: Option<String>,
    pub started: Instant,
    deadline: Option<Instant>,
    /// Last time the watched activity was seen.
    last_busy: Instant,
}

impl Session {
    /// `local_secs`: seconds since local midnight now, for [`Mode::Until`].
    pub fn new(
        mode: Mode,
        activation: Activation,
        subject: Option<String>,
        now: Instant,
        local_secs: u32,
    ) -> Self {
        let deadline = match mode {
            Mode::For(d) => Some(now + d),
            Mode::Until(m) => Some(now + until(m, local_secs)),
            _ => None,
        };
        Self {
            mode,
            activation,
            subject,
            started: now,
            deadline,
            last_busy: now,
        }
    }

    /// Time left for a timed session.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline.map(|d| d.saturating_duration_since(now))
    }

    /// Feed a sample; false once the session is over.
    pub fn tick(&mut self, now: Instant, signals: &Signals) -> bool {
        let mut quiet_for = |busy: bool, grace: Duration| {
            if busy {
                self.last_busy = now;
            }
            now.saturating_duration_since(self.last_busy) < grace
        };
        match self.mode {
            Mode::Indefinite => true,
            Mode::For(_) | Mode::Until(_) => self.remaining(now).is_some_and(|r| !r.is_zero()),
            Mode::WhileApp(_) => signals.app_running,
            Mode::WhileFileDownloading => quiet_for(signals.file_downloading, FILE_GRACE),
            Mode::WhileNetworkBusy => quiet_for(signals.down_rate >= NETWORK_BUSY, NETWORK_GRACE),
        }
    }

    /// What the session is doing: `Indefinitely`, `42:10 left`,
    /// `While Safari runs`.
    pub fn status(&self, now: Instant) -> String {
        match self.mode {
            Mode::Indefinite => "Indefinitely".into(),
            Mode::For(_) | Mode::Until(_) => {
                let left = self.remaining(now).unwrap_or_default();
                // Round up, so it never reads 0:00 while still running.
                format!(
                    "{} left",
                    crate::format::countdown(left.as_secs_f64().ceil() as u64)
                )
            }
            Mode::WhileApp(_) => format!(
                "While {} runs",
                self.subject.as_deref().unwrap_or("the app")
            ),
            Mode::WhileFileDownloading => "While a file downloads".into(),
            Mode::WhileNetworkBusy => "While the network is busy".into(),
        }
    }
}

/// Time from `local_secs` (since local midnight) to the next `minute_of_day`.
/// The same minute as now means tomorrow.
pub fn until(minute_of_day: u16, local_secs: u32) -> Duration {
    const DAY: u32 = 86_400;
    let target = (minute_of_day as u32 * 60) % DAY;
    let now = local_secs % DAY;
    let d = (target + DAY - now) % DAY;
    Duration::from_secs(if d == 0 { DAY } else { d } as u64)
}

/// Seconds since local midnight.
pub fn local_secs_of_day() -> u32 {
    let t = jiff::Zoned::now().time();
    u32::from(t.hour().unsigned_abs()) * 3600
        + u32::from(t.minute().unsigned_abs()) * 60
        + u32::from(t.second().unsigned_abs())
}

/// The current session, if any, and the running total of time kept awake.
#[derive(Debug, Default)]
pub struct KeepAwake {
    session: Option<Session>,
    total: Duration,
    /// Where the total was last counted up to.
    counted_to: Option<Instant>,
    /// The total when it was last saved.
    saved_total: Duration,
}

/// How often a running session saves the total.
pub const SAVE_EVERY: Duration = Duration::from_secs(60);

impl KeepAwake {
    pub fn with_total(total: Duration) -> Self {
        Self {
            total,
            saved_total: total,
            ..Default::default()
        }
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// All the time kept awake, including the running session so far.
    pub fn total(&self) -> Duration {
        self.total
    }

    /// The running session's status line.
    pub fn status(&self, now: Instant) -> Option<String> {
        self.session.as_ref().map(|s| s.status(now))
    }

    /// Start a session, replacing any running one.
    pub fn start(&mut self, session: Session) {
        let now = session.started;
        self.count(now);
        self.session = Some(session);
        self.counted_to = Some(now);
    }

    pub fn stop(&mut self, now: Instant) {
        self.count(now);
        self.session = None;
        self.counted_to = None;
    }

    /// Advance the session; returns whether it ended on this tick.
    pub fn tick(&mut self, now: Instant, signals: &Signals) -> bool {
        self.count(now);
        let ended = self.session.as_mut().is_some_and(|s| !s.tick(now, signals));
        if ended {
            self.stop(now);
        }
        ended
    }

    /// Whether the total has moved enough since the last save to save again
    /// (or the session just ended); marks it saved.
    pub fn take_save_due(&mut self) -> bool {
        let moved = self.total.saturating_sub(self.saved_total);
        let due = moved >= SAVE_EVERY || (!self.is_active() && !moved.is_zero());
        if due {
            self.saved_total = self.total;
        }
        due
    }

    fn count(&mut self, now: Instant) {
        if let Some(from) = self.counted_to {
            self.total += now.saturating_duration_since(from);
            self.counted_to = Some(now);
        }
    }
}

/// Starts the screen saver once each time the user has been idle for the
/// threshold, while display sleep is blocked. Re-arms on activity.
#[derive(Clone, Debug, Default)]
pub struct ScreenSaverGate {
    fired: bool,
}

impl ScreenSaverGate {
    /// True when the screen saver should start now.
    pub fn update(&mut self, idle: Duration, after: Duration) -> bool {
        if idle < after {
            self.fired = false;
            return false;
        }
        !std::mem::replace(&mut self.fired, true)
    }
}

/// What the session lets the Mac do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    pub display_sleep: bool,
    /// Sleep when the lid closes. Blocking it only works on AC power: macOS
    /// ignores the assertion on battery.
    pub closed_display_sleep: bool,
}

/// The kinds of IOKit power assertion a session takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `PreventUserIdleSystemSleep`
    IdleSystemSleep,
    /// `PreventUserIdleDisplaySleep` (also blocks the screen saver)
    IdleDisplaySleep,
    /// `PreventSystemSleep`: also blocks sleep on lid close, on AC power.
    SystemSleep,
}

impl Kind {
    /// The assertions a session needs for `allow`.
    pub fn needed(allow: Allow) -> Vec<Kind> {
        let mut v = vec![Kind::IdleSystemSleep];
        if !allow.display_sleep {
            v.push(Kind::IdleDisplaySleep);
        }
        if !allow.closed_display_sleep {
            v.push(Kind::SystemSleep);
        }
        v
    }

    #[cfg(target_os = "macos")]
    fn name(self) -> &'static std::ffi::CStr {
        match self {
            Kind::IdleSystemSleep => c"PreventUserIdleSystemSleep",
            Kind::IdleDisplaySleep => c"PreventUserIdleDisplaySleep",
            Kind::SystemSleep => c"PreventSystemSleep",
        }
    }
}

/// A held IOKit power assertion, released on drop.
#[derive(Debug)]
pub struct Assertion {
    pub kind: Kind,
    #[cfg(target_os = "macos")]
    id: u32,
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};

    pub type CFStringRef = *const c_void;
    pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    pub const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    pub const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
    pub const K_CG_ANY_INPUT_EVENT_TYPE: u32 = !0;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        pub fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        pub fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        pub fn IOPMAssertionRelease(id: u32) -> i32;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }

    /// A CFString for the call, released after it.
    pub fn with_cfstring<T>(s: &std::ffi::CStr, f: impl FnOnce(CFStringRef) -> T) -> Option<T> {
        // SAFETY: `s` is NUL-terminated UTF-8; the string is released below.
        let cf = unsafe {
            CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if cf.is_null() {
            return None;
        }
        let out = f(cf);
        // SAFETY: created above and not used after this.
        unsafe { CFRelease(cf) };
        Some(out)
    }
}

/// The name the assertions show under in `pmset -g assertions`.
pub const ASSERTION_NAME: &str = "Utilify: keep awake";

impl Assertion {
    #[cfg(target_os = "macos")]
    pub fn acquire(kind: Kind) -> anyhow::Result<Self> {
        let mut id = 0u32;
        let ret = sys::with_cfstring(kind.name(), |kind| {
            sys::with_cfstring(c"Utilify: keep awake", |name| {
                // SAFETY: both strings are live CFStrings and `id` is a valid
                // out pointer.
                unsafe {
                    sys::IOPMAssertionCreateWithName(
                        kind,
                        sys::K_IOPM_ASSERTION_LEVEL_ON,
                        name,
                        &mut id,
                    )
                }
            })
        })
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("couldn't create a CFString"))?;
        if ret != 0 {
            anyhow::bail!("IOPMAssertionCreateWithName failed ({ret:#x})");
        }
        Ok(Self { kind, id })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn acquire(_kind: Kind) -> anyhow::Result<Self> {
        anyhow::bail!("keeping awake is only supported on macOS")
    }
}

#[cfg(target_os = "macos")]
impl Drop for Assertion {
    fn drop(&mut self) {
        // SAFETY: `id` came from a successful create and is released once.
        let ret = unsafe { sys::IOPMAssertionRelease(self.id) };
        if ret != 0 {
            log::warn!("Releasing the power assertion failed ({ret:#x})");
        }
    }
}

/// The assertions currently held.
#[derive(Debug, Default)]
pub struct Assertions(Vec<Assertion>);

impl Assertions {
    /// Hold exactly `want`: release the rest, take what's missing.
    pub fn sync(&mut self, want: &[Kind]) {
        self.0.retain(|a| want.contains(&a.kind));
        for &kind in want {
            if self.0.iter().any(|a| a.kind == kind) {
                continue;
            }
            match Assertion::acquire(kind) {
                Ok(a) => self.0.push(a),
                Err(e) => log::warn!("Taking the {kind:?} assertion failed: {e}"),
            }
        }
    }
}

/// How long since the last keyboard or mouse input.
#[cfg(target_os = "macos")]
pub fn idle_time() -> Duration {
    // SAFETY: a pure query.
    let secs = unsafe {
        sys::CGEventSourceSecondsSinceLastEventType(
            sys::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM,
            sys::K_CG_ANY_INPUT_EVENT_TYPE,
        )
    };
    Duration::from_secs_f64(secs.max(0.0))
}

#[cfg(not(target_os = "macos"))]
pub fn idle_time() -> Duration {
    Duration::ZERO
}

/// Start the screen saver now.
pub fn start_screen_saver() {
    if let Err(e) = std::process::Command::new("/usr/bin/open")
        .args(["-a", "ScreenSaverEngine"])
        .spawn()
    {
        log::warn!("Starting the screen saver failed: {e}");
    }
}

/// Whether a process is still running.
pub fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks the pid.
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// The folder "While File Is Downloading" watches.
pub fn downloads_dir() -> Option<PathBuf> {
    dirs::download_dir()
}

/// Whether `dir` holds a download in progress (see [`PARTIAL_EXTENSIONS`]).
pub fn downloads_in_progress(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        path.extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| PARTIAL_EXTENSIONS.iter().any(|p| x.eq_ignore_ascii_case(p)))
    })
}

/// What `awake.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Stored {
    total_secs: u64,
}

pub fn default_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join(crate::APP_IDENTIFIER)
            .join("awake.json"),
    )
}

/// The stored total; missing or unreadable gives zero. A file that doesn't
/// parse is moved aside first ([`crate::quarantine`]).
pub fn load_total(path: &Path) -> Duration {
    let stored: Stored = crate::load_json(path, "keep-awake total").unwrap_or_default();
    Duration::from_secs(stored.total_secs)
}

/// Save atomically.
pub fn save_total(path: &Path, total: Duration) -> std::io::Result<()> {
    crate::write_atomic(
        path,
        &serde_json::to_vec_pretty(&Stored {
            total_secs: total.as_secs(),
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn session(mode: Mode, now: Instant) -> Session {
        Session::new(mode, Activation::Manual, None, now, 0)
    }

    fn rate(down_rate: f64) -> Signals {
        Signals {
            down_rate,
            ..Default::default()
        }
    }

    const IDLE: Signals = Signals {
        down_rate: 0.0,
        file_downloading: false,
        app_running: false,
    };

    #[test]
    fn timed_sessions_end_on_time() {
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        k.start(session(Mode::For(15 * MIN), t0));
        assert!(!k.tick(t0 + 10 * MIN, &IDLE));
        assert_eq!(k.session().unwrap().remaining(t0 + 10 * MIN), Some(5 * MIN));
        assert_eq!(k.status(t0 + 10 * MIN).as_deref(), Some("5:00 left"));
        assert!(k.tick(t0 + 15 * MIN, &IDLE));
        assert!(!k.is_active());
        assert_eq!(k.total(), 15 * MIN);
        // Nothing counts while off.
        assert!(!k.tick(t0 + 60 * MIN, &IDLE));
        assert_eq!(k.total(), 15 * MIN);
    }

    #[test]
    fn until_runs_to_the_next_time_of_day() {
        // 22:00 now, until 06:30: 8.5 hours.
        assert_eq!(until(6 * 60 + 30, 22 * 3600), 510 * MIN);
        // 09:00 now, until 17:00 the same day.
        assert_eq!(until(17 * 60, 9 * 3600), 8 * 60 * MIN);
        // Same minute: tomorrow.
        assert_eq!(until(9 * 60, 9 * 3600), 24 * 60 * MIN);
        let t0 = Instant::now();
        let mut s = Session::new(
            Mode::Until(9 * 60 + 5),
            Activation::Manual,
            None,
            t0,
            9 * 3600,
        );
        assert_eq!(s.remaining(t0), Some(5 * MIN));
        assert!(s.tick(t0 + 4 * MIN, &IDLE), "still running");
        assert!(!s.tick(t0 + 5 * MIN, &IDLE));
    }

    #[test]
    fn indefinite_runs_until_stopped() {
        let t0 = Instant::now();
        let mut k = KeepAwake::with_total(MIN);
        k.start(session(Mode::Indefinite, t0));
        assert!(!k.tick(t0 + 600 * MIN, &IDLE));
        k.stop(t0 + 601 * MIN);
        assert_eq!(k.total(), 602 * MIN);
    }

    #[test]
    fn network_sessions_end_after_a_quiet_spell() {
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        k.start(session(Mode::WhileNetworkBusy, t0));
        assert!(!k.tick(t0 + MIN, &IDLE), "inside the grace from the start");
        assert!(!k.tick(t0 + 3 * MIN, &rate(NETWORK_BUSY)));
        assert!(!k.tick(t0 + 4 * MIN, &rate(10.0)));
        assert!(
            k.tick(t0 + 5 * MIN, &rate(10.0)),
            "quiet for the whole grace"
        );
    }

    #[test]
    fn file_sessions_follow_the_downloads_folder() {
        let t0 = Instant::now();
        let busy = Signals {
            file_downloading: true,
            ..Default::default()
        };
        let mut s = session(Mode::WhileFileDownloading, t0);
        assert!(s.tick(t0 + 30 * MIN, &busy));
        assert!(s.tick(t0 + 30 * MIN + FILE_GRACE / 2, &IDLE));
        assert!(!s.tick(t0 + 30 * MIN + FILE_GRACE, &IDLE));
    }

    #[test]
    fn app_sessions_end_with_the_app() {
        let t0 = Instant::now();
        let mut s = Session::new(
            Mode::WhileApp(42),
            Activation::Manual,
            Some("Safari".into()),
            t0,
            0,
        );
        let running = Signals {
            app_running: true,
            ..Default::default()
        };
        assert_eq!(s.status(t0), "While Safari runs");
        assert!(s.tick(t0 + MIN, &running));
        assert!(!s.tick(t0 + 2 * MIN, &IDLE));
        assert!(process_alive(std::process::id() as i32));
        assert!(!process_alive(0));
    }

    #[test]
    fn restarting_keeps_counting() {
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        k.start(session(Mode::Indefinite, t0));
        k.start(session(Mode::For(MIN), t0 + 2 * MIN));
        assert_eq!(k.total(), 2 * MIN);
        assert!(k.tick(t0 + 3 * MIN, &IDLE));
        assert_eq!(k.total(), 3 * MIN);
    }

    #[test]
    fn saves_every_minute_and_at_the_end() {
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        assert!(!k.take_save_due());
        k.start(session(Mode::Indefinite, t0));
        k.tick(t0 + Duration::from_secs(30), &IDLE);
        assert!(!k.take_save_due());
        k.tick(t0 + Duration::from_secs(61), &IDLE);
        assert!(k.take_save_due());
        assert!(!k.take_save_due());
        k.stop(t0 + Duration::from_secs(70));
        assert!(k.take_save_due());
        assert!(!k.take_save_due());
    }

    #[test]
    fn screen_saver_fires_once_per_idle_stretch() {
        let mut g = ScreenSaverGate::default();
        let after = 45 * MIN;
        assert!(!g.update(10 * MIN, after));
        assert!(g.update(45 * MIN, after));
        assert!(!g.update(50 * MIN, after), "already started");
        assert!(!g.update(Duration::ZERO, after), "activity re-arms");
        assert!(g.update(46 * MIN, after));
    }

    #[test]
    fn assertions_follow_what_is_allowed() {
        assert_eq!(
            Kind::needed(Allow {
                display_sleep: true,
                closed_display_sleep: true
            }),
            [Kind::IdleSystemSleep]
        );
        assert_eq!(
            Kind::needed(Allow::default()),
            [
                Kind::IdleSystemSleep,
                Kind::IdleDisplaySleep,
                Kind::SystemSleep
            ]
        );
    }

    #[test]
    fn spots_partial_downloads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("done.zip"), "").unwrap();
        assert!(!downloads_in_progress(dir.path()));
        // Safari's is a folder.
        std::fs::create_dir(dir.path().join("big.zip.download")).unwrap();
        assert!(downloads_in_progress(dir.path()));
        assert!(!downloads_in_progress(&dir.path().join("missing")));
    }

    #[test]
    fn labels() {
        assert_eq!(duration_label(5 * MIN), "5 minutes");
        assert_eq!(duration_label(60 * MIN), "1 hour");
        assert_eq!(duration_label(120 * MIN), "2 hours");
        assert_eq!(duration_label(90 * MIN), "1 hour 30 minutes");
        assert_eq!(Mode::Until(18 * 60 + 5).label(), "Until 18:05");
        assert_eq!(clock(0), "00:00");
        assert_eq!(Activation::AtLaunch.label(), "Started at Launch");
    }

    /// Holds real assertions and checks macOS lists them, then that
    /// dropping releases them.
    #[cfg(target_os = "macos")]
    #[test]
    fn assertions_show_in_pmset() {
        let listed = || {
            let out = std::process::Command::new("/usr/bin/pmset")
                .args(["-g", "assertions"])
                .output()
                .unwrap();
            let pid = format!("pid {}(", std::process::id());
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.contains(&pid) && l.contains(ASSERTION_NAME))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let mut held = Assertions::default();
        held.sync(&Kind::needed(Allow::default()));
        let lines = listed();
        for kind in [
            "PreventUserIdleSystemSleep",
            "PreventUserIdleDisplaySleep",
            "PreventSystemSleep",
        ] {
            assert!(
                lines.iter().any(|l| l.contains(&format!("{kind} named"))),
                "{kind} missing: {lines:?}"
            );
        }
        held.sync(&[Kind::IdleSystemSleep]);
        assert_eq!(listed().len(), 1);
        held.sync(&[]);
        assert!(listed().is_empty());
    }

    #[test]
    fn total_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/awake.json");
        assert_eq!(load_total(&path), Duration::ZERO);
        save_total(&path, Duration::from_secs(1234)).unwrap();
        assert_eq!(load_total(&path), Duration::from_secs(1234));
        std::fs::write(&path, "junk").unwrap();
        assert_eq!(load_total(&path), Duration::ZERO);
        assert!(!path.exists());
        let aside = crate::settings::tests::only_corrupt_copy(path.parent().unwrap());
        assert_eq!(std::fs::read_to_string(aside).unwrap(), "junk");
    }
}
