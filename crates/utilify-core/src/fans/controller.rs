//! One step of fan control: given the fans, the sensors and the settings,
//! work out each fan's target and send the helper only what changed.
//!
//! The helper is only connected while some fan is manual. Going back to
//! all-automatic sends `AUTO ALL` and disconnects; the helper itself
//! restores the fans if we vanish.
//!
//! The overheating guard sits on top: if the chip stays hot while some fan
//! is held below its maximum, every fan goes back to the system's own
//! control until the chip has cooled down. Manual fans also go back when no
//! temperature can be read, since then nothing could be guarded.

use super::client::{ClientError, HelperClient};
use super::{Control, CurveFollower, FanReading, FanSettings, curve_rpm, fan_name};
use crate::sensors::{self, Sensor, SensorGroup};
use std::time::{Duration, Instant};
use utilify_smc::FanInfo;
use utilify_smc::protocol::{BUSY, Fans};

/// How often the app runs [`FanController::tick`].
pub const TICK: Duration = Duration::from_secs(2);
/// The helper refuses a second SET for one fan sooner than this.
pub const MIN_SET_INTERVAL: Duration = Duration::from_millis(250);
/// Targets closer than this to the last one sent aren't resent.
pub const DEADBAND_RPM: f32 = 25.0;
/// While the helper is reachable but unused, check on it this often.
const PROBE_READY: Duration = Duration::from_secs(5 * 60);
/// While the system is handing a fan over, ask again this soon.
const RETRY_BUSY: Duration = Duration::from_millis(500);
/// Past this, a handover still going counts as a problem worth showing.
const HANDOVER_SLOW: Duration = Duration::from_secs(45);
/// After we let fans go (or fail to take them), the SMC keeps showing them
/// manual for a moment; that's not another app.
const FOREIGN_GRACE: Duration = Duration::from_secs(10);

/// The guard trips when the hottest CPU, GPU or SoC sensor stays at or
/// above this...
pub const GUARD_TRIP_C: f32 = 95.0;
/// ...for this long (single cores spike past it for a moment under load).
pub const GUARD_TRIP_AFTER: Duration = Duration::from_secs(10);
/// It lets go once the chip is below this, and no sooner than
/// [`GUARD_HOLD`] after tripping, so it doesn't flap.
pub const GUARD_RELEASE_C: f32 = 80.0;
pub const GUARD_HOLD: Duration = Duration::from_secs(60);

/// Whether fan control is available, for the UI's banner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HelperStatus {
    /// Not checked yet.
    #[default]
    Unknown,
    /// Not an Apple Silicon Mac: fans are read-only.
    Unsupported,
    NotInstalled,
    NotRunning,
    PermissionDenied,
    /// Reachable.
    Ready,
    /// It went away mid-conversation or answered nonsense.
    Broken(String),
}

impl HelperStatus {
    pub fn is_ready(&self) -> bool {
        *self == HelperStatus::Ready
    }

    fn from_error(e: &ClientError) -> Self {
        match e {
            ClientError::NotInstalled => HelperStatus::NotInstalled,
            ClientError::NotRunning => HelperStatus::NotRunning,
            ClientError::PermissionDenied => HelperStatus::PermissionDenied,
            // A refusal still means it's there.
            ClientError::Refused(_) => HelperStatus::Ready,
            ClientError::Broken(m) => HelperStatus::Broken(m.clone()),
        }
    }
}

/// One fan as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FanStatus {
    pub info: FanInfo,
    pub name: String,
    pub control: Control,
    /// What we asked the helper for; `None` while automatic.
    pub target: Option<f32>,
    /// Why the control isn't in effect, e.g. its sensor is missing.
    pub issue: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sent {
    Auto,
    Rpm(f32),
    /// Asked for manual; the helper holds the fan while the system hands it
    /// over.
    Pending,
}

struct Slot {
    control: Control,
    follower: CurveFollower,
    sent: Sent,
    last_set: Option<Instant>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            control: Control::Auto,
            follower: CurveFollower::default(),
            sent: Sent::Auto,
            last_set: None,
        }
    }
}

/// The overheating guard has the fans on automatic.
#[derive(Clone, Debug, PartialEq)]
pub struct Overheat {
    /// The hottest watched sensor, as last read.
    pub sensor: String,
    pub celsius: f32,
}

/// What a tick found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickReport {
    pub fans: Vec<FanStatus>,
    /// Set while the overheating guard overrides the controls.
    pub overheat: Option<Overheat>,
    /// A change was held back by the helper's rate limit: tick again this
    /// soon.
    pub retry_in: Option<Duration>,
}

pub struct FanController {
    client: HelperClient,
    /// By position in the fan list.
    slots: Vec<Slot>,
    last_tick: Option<Instant>,
    status: HelperStatus,
    /// The helper's last refusal, until a request succeeds.
    error: Option<String>,
    last_probe: Option<Instant>,
    /// Fan control is possible on this Mac at all.
    supported: bool,
    /// Since when the chip has been at [`GUARD_TRIP_C`] or more.
    hot_since: Option<Instant>,
    /// When the guard tripped, and the latest hot reading.
    tripped: Option<(Instant, Overheat)>,
    /// Since when some fan has been [`Sent::Pending`].
    pending_since: Option<Instant>,
    /// Manual fans or `Ftst` aren't another app's before this.
    quiet_until: Option<Instant>,
}

impl FanController {
    pub fn new(client: HelperClient, supported: bool) -> Self {
        Self {
            client,
            slots: Vec::new(),
            last_tick: None,
            status: if supported {
                HelperStatus::Unknown
            } else {
                HelperStatus::Unsupported
            },
            error: None,
            last_probe: None,
            supported,
            hot_since: None,
            tripped: None,
            pending_since: None,
            quiet_until: None,
        }
    }

    pub fn status(&self) -> &HelperStatus {
        &self.status
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether we hold (or are taking) any fan.
    pub fn holding(&self) -> bool {
        self.slots.iter().any(|s| s.sent != Sent::Auto)
    }

    /// How many fans we hold in manual mode.
    pub fn held(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s.sent, Sent::Rpm(_)))
            .count()
    }

    /// Some fan is waiting for the system to hand it over.
    pub fn taking_over(&self) -> bool {
        self.slots.iter().any(|s| s.sent == Sent::Pending)
    }

    /// Something other than us has the fans: `Ftst` set while we hold
    /// nothing, or a fan manual that isn't ours. Not straight after we let
    /// go, while the SMC catches up.
    pub fn foreign_control(&self, reading: &FanReading, now: Instant) -> bool {
        if self.quiet_until.is_some_and(|t| now < t) {
            return false;
        }
        let ours = |i: usize| self.slots.get(i).is_some_and(|s| s.sent != Sent::Auto);
        (reading.forced && !self.holding())
            || reading
                .fans
                .iter()
                .enumerate()
                .any(|(i, f)| f.manual && !ours(i))
    }

    /// The user just changed something: try the helper again straight away
    /// instead of waiting out a backoff.
    pub fn retry_now(&mut self) {
        self.client.retry_now();
    }

    /// Reads nothing itself: `reading` and `sensors` come from the caller,
    /// so this runs anywhere (and in tests).
    pub fn tick(
        &mut self,
        reading: &FanReading,
        sensors: &[Sensor],
        settings: &FanSettings,
        now: Instant,
    ) -> TickReport {
        let dt = self
            .last_tick
            .map_or(TICK, |t| now.saturating_duration_since(t))
            .min(TICK * 5);
        self.last_tick = Some(now);
        let fans = &reading.fans;
        let controls = if self.supported {
            settings.resolve(fans)
        } else {
            vec![Control::Auto; fans.len()]
        };
        self.slots.resize_with(fans.len(), Slot::default);

        let mut report = TickReport::default();
        let mut wanted = Vec::with_capacity(fans.len());
        for ((fan, control), slot) in fans.iter().zip(&controls).zip(&mut self.slots) {
            if slot.control != *control {
                slot.control = control.clone();
                slot.follower = CurveFollower::default();
            }
            let mut issue = None;
            let want = match control {
                Control::Auto => None,
                Control::Constant(rpm) => Some(rpm.clamp(fan.min, fan.max)),
                Control::SensorCurve {
                    sensor_id,
                    from_c,
                    to_c,
                } => match sensors::find(sensors, sensor_id) {
                    Some(s) => {
                        let raw = curve_rpm(fan.min, fan.max, *from_c, *to_c, s.celsius);
                        let start = fan.current.clamp(fan.min, fan.max);
                        Some(slot.follower.step(raw, s.celsius, start, dt))
                    }
                    // Without its sensor the curve can't run; the system's
                    // own control is the safe fallback.
                    None => {
                        issue = Some(format!("sensor {sensor_id} not found; running automatic"));
                        None
                    }
                },
            };
            wanted.push(want);
            report.fans.push(FanStatus {
                info: *fan,
                name: fan_name(fan.index, fans.len()),
                control: control.clone(),
                target: want,
                issue,
            });
        }

        let hottest = watched(sensors);
        report.overheat = self.guard(hottest, fans, &wanted, now);
        if wanted.iter().any(Option::is_some) && (report.overheat.is_some() || hottest.is_none()) {
            let why = match &report.overheat {
                Some(o) => format!(
                    "{} at {:.0}°C: automatic until it's below {GUARD_RELEASE_C:.0}°C",
                    o.sensor, o.celsius
                ),
                None => "no temperature readings; running automatic".into(),
            };
            for (want, status) in wanted.iter_mut().zip(&mut report.fans) {
                if want.take().is_some() {
                    status.target = None;
                    status.issue = Some(why.clone());
                }
            }
        }

        if wanted.iter().any(Option::is_some) {
            report.retry_in = self.drive(fans, &wanted, now);
        } else if self.let_go(now) {
            // It was just there.
            self.last_probe = Some(now);
        } else {
            self.probe(now);
        }
        if !self.status.is_ready() {
            for f in &mut report.fans {
                f.target = None;
            }
        }
        report
    }

    /// Tracks how long the chip has been hot, trips when it has been for
    /// [`GUARD_TRIP_AFTER`] while some fan is wanted below its maximum (at
    /// the maximum the system couldn't cool it any better), and lets go
    /// once it's cool again. `Some` while tripped.
    fn guard(
        &mut self,
        hottest: Option<&Sensor>,
        fans: &[FanInfo],
        wanted: &[Option<f32>],
        now: Instant,
    ) -> Option<Overheat> {
        let reading = hottest.map(|s| Overheat {
            sensor: s.name.clone(),
            celsius: s.celsius,
        });
        if let Some((since, last)) = &mut self.tripped {
            match &reading {
                Some(r) if r.celsius < GUARD_RELEASE_C
                    && now.saturating_duration_since(*since) >= GUARD_HOLD =>
                {
                    log::info!("Cooled to {:.0}°C: fan controls back on", r.celsius);
                    self.tripped = None;
                }
                Some(r) => *last = r.clone(),
                // Nothing to go by: stay tripped.
                None => {}
            }
        }
        match &reading {
            Some(r) if r.celsius >= GUARD_TRIP_C => {
                self.hot_since.get_or_insert(now);
            }
            _ => self.hot_since = None,
        }
        let held_low = fans
            .iter()
            .zip(wanted)
            .any(|(f, w)| w.is_some_and(|rpm| rpm < f.max - DEADBAND_RPM));
        if self.tripped.is_none()
            && held_low
            && let (Some(t), Some(r)) = (self.hot_since, reading)
            && now.saturating_duration_since(t) >= GUARD_TRIP_AFTER
        {
            log::warn!(
                "{} at {:.0}°C: fans back to automatic until it's below {GUARD_RELEASE_C:.0}°C",
                r.sensor,
                r.celsius
            );
            self.tripped = Some((now, r));
        }
        self.tripped.as_ref().map(|(_, r)| r.clone())
    }

    /// Sends what changed. Returns when to retry a rate-limited fan.
    fn drive(
        &mut self,
        fans: &[FanInfo],
        wanted: &[Option<f32>],
        now: Instant,
    ) -> Option<Duration> {
        if let Err(e) = self.client.ensure_connected(now) {
            self.lost(&e);
            return None;
        }
        self.status = HelperStatus::Ready;
        let mut retry: Option<Duration> = None;
        // One fan still being handed over holds the rest up too: don't
        // spend this tick waiting on each in turn.
        let mut waiting = false;
        for (i, (fan, want)) in fans.iter().zip(wanted).enumerate() {
            let slot = &mut self.slots[i];
            let result = match (*want, slot.sent) {
                (None, Sent::Rpm(_) | Sent::Pending) => {
                    self.quiet_until = Some(now + FOREIGN_GRACE);
                    self.client.auto(Fans::One(fan.index)).map(|()| {
                        slot.sent = Sent::Auto;
                    })
                }
                (Some(rpm), sent) => {
                    if waiting
                        || matches!(sent, Sent::Rpm(r) if (r - rpm).abs() < DEADBAND_RPM)
                    {
                        continue;
                    }
                    if let Some(t) = slot.last_set
                        && now.saturating_duration_since(t) < MIN_SET_INTERVAL
                    {
                        let wait = MIN_SET_INTERVAL - now.saturating_duration_since(t);
                        retry = Some(retry.map_or(wait, |r| r.min(wait)));
                        continue;
                    }
                    slot.last_set = Some(now);
                    self.client.set(fan.index, rpm).map(|_applied| {
                        // Compare later targets against what we asked for;
                        // the applied speed is the same, clamped.
                        slot.sent = Sent::Rpm(rpm);
                    })
                }
                (None, Sent::Auto) => continue,
            };
            if !matches!(result, Err(ClientError::Broken(_))) {
                self.client.touch(now);
            }
            match result {
                Ok(()) => self.error = None,
                Err(ClientError::Refused(msg)) if msg.starts_with("too fast") => {
                    retry = Some(MIN_SET_INTERVAL);
                }
                Err(ClientError::Refused(msg)) if msg.starts_with(BUSY) => {
                    self.slots[i].sent = Sent::Pending;
                    self.quiet_until = Some(now + FOREIGN_GRACE);
                    waiting = true;
                    retry = Some(RETRY_BUSY);
                    let since = *self.pending_since.get_or_insert(now);
                    if now.saturating_duration_since(since) >= HANDOVER_SLOW {
                        self.error = Some(format!(
                            "macOS still hasn't handed over the fans after {} s; still trying",
                            now.saturating_duration_since(since).as_secs()
                        ));
                    }
                }
                Err(ClientError::Refused(msg)) => {
                    log::warn!("Fan helper refused fan {}: {msg}", fan.index);
                    self.error = Some(msg);
                }
                Err(e) => {
                    self.lost(&e);
                    return None;
                }
            }
        }
        if !self.taking_over() {
            self.pending_since = None;
        }
        if let Err(e) = self.client.keep_alive(now) {
            self.lost(&e);
        }
        retry
    }

    /// The connection is gone, and with it (the helper makes sure) any
    /// manual fans.
    fn lost(&mut self, e: &ClientError) {
        if self.client.is_connected() || self.holding() {
            log::warn!("Fan control unavailable: {e}");
        }
        self.client.disconnect();
        self.status = HelperStatus::from_error(e);
        for s in &mut self.slots {
            s.sent = Sent::Auto;
        }
        self.pending_since = None;
    }

    /// Nothing manual is wanted: hand every fan back and disconnect. True
    /// if there was a connection.
    fn let_go(&mut self, now: Instant) -> bool {
        let connected = self.client.is_connected();
        self.pending_since = None;
        if self.holding() {
            self.quiet_until = Some(now + FOREIGN_GRACE);
        }
        if connected {
            if let Err(e) = self.client.auto(Fans::All) {
                log::warn!("Returning the fans to automatic failed: {e}");
            }
            self.client.disconnect();
        }
        for s in &mut self.slots {
            s.sent = Sent::Auto;
        }
        connected
    }

    /// Keeps [`HelperStatus`] current while nothing is connected: often
    /// while it's unavailable (a missing socket costs nothing to check, and
    /// backoff limits it), rarely once it's known to be there.
    fn probe(&mut self, now: Instant) {
        if !self.supported {
            return;
        }
        let due = match self.last_probe {
            None => true,
            Some(_) if !self.status.is_ready() => true,
            Some(t) => now.saturating_duration_since(t) >= PROBE_READY,
        };
        if !due {
            return;
        }
        self.last_probe = Some(now);
        self.status = match self.client.probe(now) {
            Ok(()) => HelperStatus::Ready,
            Err(e) => HelperStatus::from_error(&e),
        };
    }

    /// On quit: every fan back to automatic now, rather than when the
    /// helper notices we're gone.
    pub fn release(&mut self) {
        if self.client.is_connected() {
            log::info!("Returning the fans to automatic");
        }
        self.let_go(Instant::now());
    }
}

/// The hottest CPU, GPU or SoC sensor: what the guard watches.
fn watched(sensors: &[Sensor]) -> Option<&Sensor> {
    sensors
        .iter()
        .filter(|s| matches!(s.group, SensorGroup::Cpu | SensorGroup::Gpu | SensorGroup::Soc))
        .max_by(|a, b| a.celsius.total_cmp(&b.celsius))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fans::client::tests::FakeHelper;
    use crate::fans::{Active, FanSettings};

    fn fan(index: u8, current: f32) -> FanInfo {
        FanInfo {
            index,
            current,
            min: 1000.0,
            max: 5000.0,
            target: current,
            manual: false,
        }
    }

    fn reading() -> FanReading {
        FanReading {
            fans: vec![fan(0, 1200.0), fan(1, 1200.0)],
            forced: false,
        }
    }

    fn cpu(c: f32) -> Vec<Sensor> {
        vec![Sensor {
            id: sensors::CPU_AVERAGE_ID.into(),
            name: "CPU Core Average".into(),
            group: SensorGroup::Cpu,
            celsius: c,
        }]
    }

    fn custom(controls: Vec<Control>) -> FanSettings {
        FanSettings {
            active: Active::Custom,
            custom: controls,
            ..Default::default()
        }
    }

    fn controller(h: &FakeHelper) -> FanController {
        FanController::new(HelperClient::new(h.paths()), true)
    }

    #[test]
    fn sends_only_changes_and_lets_go_when_automatic() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let t = Instant::now();
        let auto = FanSettings::default();
        // Automatic: a probe, nothing held.
        let r = c.tick(&reading(), &cpu(50.0), &auto, t);
        assert_eq!(r.fans[0].name, "Left side");
        assert_eq!(c.status(), &HelperStatus::Ready);
        assert_eq!(h.wait_for(2), ["HELLO 1", "<closed>"]);

        let s = custom(vec![Control::Constant(3000.0), Control::Constant(9000.0)]);
        let r = c.tick(&reading(), &cpu(50.0), &s, t + TICK);
        assert_eq!(r.fans[1].target, Some(5000.0), "clamped to the range");
        assert!(c.holding());
        // Same again: nothing sent.
        c.tick(&reading(), &cpu(50.0), &s, t + 2 * TICK);
        // One fan back to auto.
        let s = custom(vec![Control::Constant(3000.0), Control::Auto]);
        c.tick(&reading(), &cpu(50.0), &s, t + 3 * TICK);
        // Back to Automatic: AUTO ALL and disconnect.
        c.tick(&reading(), &cpu(50.0), &auto, t + 4 * TICK);
        assert!(!c.holding());
        assert_eq!(
            h.wait_for(8),
            [
                "HELLO 1",
                "<closed>",
                "HELLO 1",
                "SET 0 3000",
                "SET 1 5000",
                "AUTO 1",
                "AUTO ALL",
                "<closed>"
            ]
        );
    }

    #[test]
    fn respects_the_rate_limit_and_pings() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let t = Instant::now();
        c.tick(&reading(), &cpu(50.0), &custom(vec![Control::Constant(2000.0)]), t);
        // A quick change from the UI, 100 ms later: held back.
        let s = custom(vec![Control::Constant(2500.0)]);
        let r = c.tick(&reading(), &cpu(50.0), &s, t + Duration::from_millis(100));
        assert_eq!(r.retry_in, Some(Duration::from_millis(150)));
        let r = c.tick(&reading(), &cpu(50.0), &s, t + Duration::from_millis(250));
        assert_eq!(r.retry_in, None);
        // Nothing new for 5 s: a PING keeps the helper from timing us out.
        c.tick(&reading(), &cpu(50.0), &s, t + Duration::from_secs(6));
        assert_eq!(
            h.wait_for(4),
            ["HELLO 1", "SET 0 2000", "SET 0 2500", "PING"]
        );
    }

    #[test]
    fn follows_a_sensor_with_hysteresis() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let s = custom(vec![Control::SensorCurve {
            sensor_id: sensors::CPU_AVERAGE_ID.into(),
            from_c: 40.0,
            to_c: 80.0,
        }]);
        let t = Instant::now();
        // 60 °C is halfway: 3000 rpm, ramping from 1200 at 1000 rpm/s.
        let r = c.tick(&reading(), &cpu(60.0), &s, t);
        assert_eq!(r.fans[0].target, Some(3000.0));
        let r = c.tick(&reading(), &cpu(59.0), &s, t + TICK);
        assert_eq!(r.fans[0].target, Some(3000.0), "within the hysteresis");
        let r = c.tick(&reading(), &cpu(50.0), &s, t + 2 * TICK);
        assert_eq!(r.fans[0].target, Some(2500.0), "down at 250 rpm/s");
        // The sensor disappears: automatic, with a reason.
        let r = c.tick(&reading(), &[], &s, t + 3 * TICK);
        assert_eq!(r.fans[0].target, None);
        assert!(r.fans[0].issue.as_deref().unwrap().contains("not found"));
        assert_eq!(
            h.wait_for(5),
            [
                "HELLO 1",
                "SET 0 3000",
                "SET 0 2500",
                "AUTO ALL",
                "<closed>"
            ]
        );
    }

    #[test]
    fn reports_refusals_and_missing_helper() {
        let h = FakeHelper::start(|line| {
            Some(if line.starts_with("HELLO") {
                "OK 1 2".into()
            } else {
                "ERR the SMC didn't take manual mode".into()
            })
        });
        let mut c = controller(&h);
        let s = custom(vec![Control::Constant(2000.0)]);
        let r = c.tick(&reading(), &cpu(50.0), &s, Instant::now());
        assert_eq!(c.status(), &HelperStatus::Ready);
        assert_eq!(c.error(), Some("the SMC didn't take manual mode"));
        assert!(!c.holding());
        assert_eq!(r.fans[0].target, Some(2000.0));

        let dir = tempfile::tempdir().unwrap();
        let mut c = FanController::new(
            HelperClient::new(crate::fans::client::HelperPaths {
                socket: dir.path().join("none.sock"),
                plist: Some(dir.path().join("none.plist")),
            }),
            true,
        );
        let r = c.tick(&reading(), &cpu(50.0), &s, Instant::now());
        assert_eq!(c.status(), &HelperStatus::NotInstalled);
        assert_eq!(r.fans[0].target, None);
        let r = c.tick(&reading(), &cpu(50.0), &FanSettings::default(), Instant::now());
        assert_eq!(c.status(), &HelperStatus::NotInstalled);
        assert_eq!(r.fans.len(), 2);
    }

    #[test]
    fn unsupported_macs_stay_automatic() {
        let h = FakeHelper::two_fans();
        let mut c = FanController::new(HelperClient::new(h.paths()), false);
        let s = custom(vec![Control::Constant(2000.0)]);
        let r = c.tick(&reading(), &cpu(50.0), &s, Instant::now());
        assert_eq!(r.fans[0].control, Control::Auto);
        assert_eq!(c.status(), &HelperStatus::Unsupported);
        std::thread::sleep(Duration::from_millis(20));
        assert!(h.lines.lock().unwrap().is_empty(), "never connected");
    }

    #[test]
    fn release_sends_auto_all() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        c.tick(
            &reading(),
            &cpu(50.0),
            &custom(vec![Control::Constant(2000.0)]),
            Instant::now(),
        );
        c.release();
        assert_eq!(
            h.wait_for(4),
            ["HELLO 1", "SET 0 2000", "AUTO ALL", "<closed>"]
        );
        // Nothing held: release sends nothing.
        c.release();
    }

    #[test]
    fn guard_hands_hot_fans_back_until_cool() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let s = custom(vec![Control::Constant(2000.0)]);
        let t = Instant::now();
        let r = c.tick(&reading(), &cpu(97.0), &s, t);
        assert_eq!(r.fans[0].target, Some(2000.0), "a moment's heat is fine");
        assert_eq!(r.overheat, None);
        let r = c.tick(&reading(), &cpu(97.0), &s, t + GUARD_TRIP_AFTER);
        assert_eq!(r.fans[0].target, None);
        assert!(r.fans[0].issue.as_deref().unwrap().contains("97°C"));
        assert_eq!(r.overheat.as_ref().unwrap().celsius, 97.0);
        assert!(!c.holding());
        // Cool, but not for long enough yet; then warm, but not below the
        // release temperature.
        let r = c.tick(&reading(), &cpu(70.0), &s, t + GUARD_TRIP_AFTER + TICK);
        assert!(r.overheat.is_some());
        let later = t + GUARD_TRIP_AFTER + GUARD_HOLD;
        let r = c.tick(&reading(), &cpu(85.0), &s, later);
        assert_eq!(r.overheat.as_ref().unwrap().celsius, 85.0);
        // Cool and past the hold: the control is back.
        let r = c.tick(&reading(), &cpu(70.0), &s, later + TICK);
        assert_eq!(r.overheat, None);
        assert_eq!(r.fans[0].target, Some(2000.0));
        assert_eq!(
            h.wait_for(6),
            [
                "HELLO 1",
                "SET 0 2000",
                "AUTO ALL",
                "<closed>",
                "HELLO 1",
                "SET 0 2000"
            ]
        );
    }

    #[test]
    fn guard_leaves_fans_at_their_maximum() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let full = FanSettings {
            active: Active::FullBlast,
            ..Default::default()
        };
        let t = Instant::now();
        c.tick(&reading(), &cpu(99.0), &full, t);
        let r = c.tick(&reading(), &cpu(99.0), &full, t + 2 * GUARD_TRIP_AFTER);
        assert_eq!(r.overheat, None);
        assert_eq!(r.fans[0].target, Some(5000.0));
    }

    #[test]
    fn manual_needs_a_temperature() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let s = custom(vec![Control::Constant(2000.0)]);
        let r = c.tick(&reading(), &[], &s, Instant::now());
        assert_eq!(r.fans[0].target, None);
        assert!(r.fans[0].issue.as_deref().unwrap().contains("no temperature"));
        assert!(!c.holding());
    }

    #[test]
    fn spots_other_tools() {
        let h = FakeHelper::two_fans();
        let mut c = controller(&h);
        let mut r = reading();
        let t = Instant::now();
        assert!(!c.foreign_control(&r, t));
        r.forced = true;
        assert!(c.foreign_control(&r, t));
        r.forced = false;
        r.fans[1].manual = true;
        assert!(c.foreign_control(&r, t));
        // Ours: not foreign.
        let s = custom(vec![Control::Auto, Control::Constant(2000.0)]);
        c.tick(&r, &cpu(50.0), &s, t);
        r.forced = true;
        assert!(!c.foreign_control(&r, t));
        // Just let go: the SMC still shows them manual for a moment.
        c.tick(&r, &cpu(50.0), &FanSettings::default(), t);
        assert!(!c.foreign_control(&r, t + Duration::from_secs(2)));
        assert!(c.foreign_control(&r, t + FOREIGN_GRACE));
    }

    /// Answers SET with `busy:` the first `busy` times.
    fn slow_handover(busy: u32) -> FakeHelper {
        let left = std::sync::atomic::AtomicU32::new(busy);
        FakeHelper::start(move |line| {
            Some(match utilify_smc::protocol::Request::parse(line) {
                Ok(utilify_smc::protocol::Request::Hello(_)) => "OK 1 2".into(),
                Ok(utilify_smc::protocol::Request::Set { rpm, .. }) => {
                    let n = left.load(std::sync::atomic::Ordering::SeqCst);
                    if n > 0 {
                        left.store(n - 1, std::sync::atomic::Ordering::SeqCst);
                        "ERR busy: the SMC didn't take manual mode yet".into()
                    } else {
                        format!("OK {rpm}")
                    }
                }
                _ => "OK".into(),
            })
        })
    }

    #[test]
    fn waits_out_the_handover_without_an_error() {
        let h = slow_handover(2);
        let mut c = controller(&h);
        let s = FanSettings {
            active: Active::FullBlast,
            ..Default::default()
        };
        let t = Instant::now();
        let r = c.tick(&reading(), &cpu(50.0), &s, t);
        assert!(c.taking_over());
        assert_eq!(c.error(), None);
        assert_eq!(r.retry_in, Some(RETRY_BUSY));
        assert!(!c.foreign_control(&FanReading { forced: true, ..reading() }, t));
        c.tick(&reading(), &cpu(50.0), &s, t + Duration::from_secs(1));
        assert!(c.taking_over());
        c.tick(&reading(), &cpu(50.0), &s, t + Duration::from_secs(2));
        assert!(!c.taking_over());
        assert_eq!(c.held(), 2);
        // Fan 1 waited while fan 0 was being handed over.
        assert_eq!(
            h.wait_for(5),
            ["HELLO 1", "SET 0 5000", "SET 0 5000", "SET 0 5000", "SET 1 5000"]
        );
    }

    #[test]
    fn a_slow_handover_is_shown_eventually() {
        let h = slow_handover(u32::MAX);
        let mut c = controller(&h);
        let s = custom(vec![Control::Constant(2000.0)]);
        let t = Instant::now();
        c.tick(&reading(), &cpu(50.0), &s, t);
        assert_eq!(c.error(), None);
        c.tick(&reading(), &cpu(50.0), &s, t + HANDOVER_SLOW);
        assert!(c.error().unwrap().contains("still trying"));
    }

    #[test]
    fn letting_go_mid_handover_releases_the_fan() {
        let h = slow_handover(u32::MAX);
        let mut c = controller(&h);
        let t = Instant::now();
        let both = custom(vec![Control::Constant(2000.0), Control::Constant(2000.0)]);
        c.tick(&reading(), &cpu(50.0), &both, t);
        // Fan 0 back to automatic while it's still being handed over.
        let one = custom(vec![Control::Auto, Control::Constant(2000.0)]);
        c.tick(&reading(), &cpu(50.0), &one, t + Duration::from_secs(1));
        c.tick(&reading(), &cpu(50.0), &FanSettings::default(), t + Duration::from_secs(2));
        assert!(!c.holding());
        assert_eq!(
            h.wait_for(6),
            ["HELLO 1", "SET 0 2000", "AUTO 0", "SET 1 2000", "AUTO ALL", "<closed>"]
        );
    }

    /// Against the real helper binary with its in-memory SMC (never the
    /// hardware): `cargo build -p utilify-fanhelper && cargo test -p
    /// utilify-core real_helper -- --ignored`.
    #[test]
    #[ignore]
    fn real_helper() {
        let bin = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/debug/utilify-fanhelper"
        );
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("helper.sock");
        let mut child = std::process::Command::new(bin)
            .args(["--fake", "--no-root-check", "--socket"])
            .arg(&socket)
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("build utilify-fanhelper first");
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let paths = crate::fans::client::HelperPaths {
            socket,
            plist: None,
        };
        let mut c = FanController::new(HelperClient::new(paths), true);
        let t = Instant::now();
        let s = custom(vec![Control::Constant(9000.0), Control::Constant(2000.0)]);
        c.tick(&reading(), &cpu(50.0), &s, t);
        assert_eq!(c.status(), &HelperStatus::Ready);
        assert_eq!(c.error(), None);
        assert!(c.holding());
        c.tick(&reading(), &cpu(50.0), &FanSettings::default(), t + TICK);
        assert!(!c.holding());
        assert_eq!(c.error(), None);
        let _ = child.kill();
        let _ = child.wait();
    }
}
