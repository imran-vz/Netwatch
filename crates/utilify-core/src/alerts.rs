//! When to notify. Each tick the app gathers an [`AlertInputs`] and calls
//! [`Alerts::tick`], which returns what to post and what to take back.
//! Pure and deterministic: time comes in with the inputs and nothing here
//! touches the OS, so every rule is tested with made-up clocks.
//!
//! Every rule has its own small state machine, fed on every tick whether
//! its notification is enabled or not (like [`StorageAlarm`]), so turning
//! one on later doesn't fire for something that happened while it was off.
//! The engine sits on top: it drops posts for disabled or snoozed kinds,
//! remembers which posts are still showing, and retracts them when their
//! condition clears or their kind is turned off.

use crate::fans::Active;
use crate::fans::controller::{FanStatus, GUARD_RELEASE_C, HelperStatus, Overheat};
use crate::fans::service::FanSnapshot;
use crate::power::PowerState;
use crate::settings::StorageAlarm;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const HOUR: Duration = Duration::from_secs(60 * 60);

/// Fans forced high on battery this long before [`AlertKind::FansHighOnBattery`]...
pub const FANS_HIGH_AFTER: Duration = Duration::from_secs(10 * 60);
/// ...which repeats this often while it lasts.
pub const FANS_HIGH_REPEAT: Duration = HOUR;
/// A fan counts as forced high from this share of its range above its
/// minimum (see [`forced_high`]).
pub const FORCED_HIGH_FRACTION: f32 = 0.8;
/// [`AlertKind::LowBatteryAwake`] re-arms once the battery is this many
/// points above the low threshold (or charging).
pub const LOW_BATTERY_REARM: u8 = 5;
/// [`AlertKind::LidSleepOnBattery`] posts at most this often, however much
/// the power flaps.
pub const LID_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// [`AlertKind::AwakeEndingSoon`] only for sessions longer than this...
pub const ENDING_MIN_LENGTH: Duration = Duration::from_secs(15 * 60);
/// ...when this much is left.
pub const ENDING_WARN: Duration = Duration::from_secs(5 * 60);
/// Thermal pressure must last this long to notify...
pub const THERMAL_AFTER: Duration = Duration::from_secs(30);
/// ...and a cooler state this long to take it back.
pub const THERMAL_CLEAR_AFTER: Duration = Duration::from_secs(60);
/// A new thermal episode notifies at most this often (escalating within
/// one always does).
pub const THERMAL_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// The helper must be gone this long to notify.
pub const HELPER_LOST_AFTER: Duration = Duration::from_secs(10);
/// Another app must hold the fans this long to notify.
pub const FOREIGN_AFTER: Duration = Duration::from_secs(5);
/// Offline this long to notify...
pub const OFFLINE_AFTER: Duration = Duration::from_secs(60);
/// ...and at most this often.
pub const OFFLINE_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Storage crossing into red notifies at most this often.
pub const STORAGE_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
/// How long [`Alerts::snooze`] silences a kind.
pub const SNOOZE: Duration = HOUR;

/// Action: put every fan back on automatic.
pub const ACTION_FANS_AUTO: &str = "fans-auto";
/// Action: [`Alerts::snooze`] this kind.
pub const ACTION_SNOOZE_1H: &str = "snooze-1h";
/// Action: end the keep-awake session.
pub const ACTION_AWAKE_END: &str = "awake-end";
/// Action: add 30 minutes to the keep-awake session.
pub const ACTION_AWAKE_EXTEND: &str = "awake-extend";
/// Action: open the dashboard's Fans tab.
pub const ACTION_OPEN_FANS: &str = "open-fans";

/// The system's thermal state, as `NSProcessInfo.thermalState` reports it.
/// The app maps gpui's to this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Thermal {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
}

/// The fan helper, as far as alerts care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HelperState {
    Ready,
    /// Installed (or not known yet) but not answering.
    Lost,
    /// Not installed, or fans can't be set on this Mac.
    #[default]
    NotInstalled,
}

impl HelperState {
    /// Collapses the controller's [`HelperStatus`].
    pub fn from_status(status: &HelperStatus) -> Self {
        match status {
            HelperStatus::Ready => HelperState::Ready,
            HelperStatus::NotInstalled | HelperStatus::Unsupported => HelperState::NotInstalled,
            HelperStatus::Unknown
            | HelperStatus::NotRunning
            | HelperStatus::PermissionDenied
            | HelperStatus::Broken(_) => HelperState::Lost,
        }
    }
}

/// Whether the fans are forced high: the Full blast preset is active, or
/// some fan we command has a target at or above [`FORCED_HIGH_FRACTION`]
/// of its range above its minimum (for a 1200–5700 rpm fan, 4800 rpm or
/// more). A fan with no usable range counts when its target is at its
/// maximum.
pub fn forced_high(active: &Active, fans: &[FanStatus]) -> bool {
    if *active == Active::FullBlast {
        return true;
    }
    fans.iter().any(|f| {
        let Some(target) = f.target else { return false };
        let (min, max) = (f.info.min, f.info.max);
        if max <= min {
            return target >= max;
        }
        target >= min + (max - min) * FORCED_HIGH_FRACTION
    })
}

/// What the fans are doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanInputs {
    /// We command at least one fan (it has a target).
    pub manual: bool,
    /// See [`forced_high`].
    pub forced_high: bool,
    pub helper: HelperState,
    /// The overheating guard has the fans on automatic.
    pub overheat: Option<Overheat>,
    /// Another app holds the fans.
    pub foreign: bool,
}

impl FanInputs {
    /// From the fan thread's latest snapshot and the active preset.
    pub fn from_snapshot(snapshot: &FanSnapshot, active: &Active) -> Self {
        let manual = snapshot.fans.iter().any(|f| f.target.is_some());
        Self {
            manual,
            forced_high: manual && forced_high(active, &snapshot.fans),
            helper: HelperState::from_status(&snapshot.helper),
            overheat: snapshot.overheat.clone(),
            foreign: snapshot.foreign,
        }
    }
}

/// The keep-awake session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AwakeInputs {
    pub active: bool,
    /// Time left, for a timed session.
    pub remaining: Option<Duration>,
    /// The timed session's whole length.
    pub length: Option<Duration>,
    /// The session blocks sleep when the lid closes (which only holds on
    /// AC power).
    pub closed_display_sleep_blocked: bool,
}

impl AwakeInputs {
    /// From the running session (if any) and what sessions allow.
    pub fn from_keep_awake(
        keep_awake: &crate::awake::KeepAwake,
        allow: crate::awake::Allow,
        now: Instant,
    ) -> Self {
        let Some(s) = keep_awake.session() else {
            return Self::default();
        };
        let remaining = s.remaining(now);
        Self {
            active: true,
            remaining,
            length: remaining.map(|r| r + now.saturating_duration_since(s.started)),
            closed_display_sleep_blocked: !allow.closed_display_sleep,
        }
    }
}

/// The startup disk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StorageInputs {
    /// Used share, 0..=1; `None` when unreadable this tick.
    pub used: Option<f64>,
    /// The red threshold, 0..=1 ([`crate::settings::Settings::storage_thresholds`]).
    pub red: f64,
}

impl Default for StorageInputs {
    fn default() -> Self {
        Self {
            used: None,
            red: 0.8,
        }
    }
}

/// Everything the rules look at, gathered once per tick.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertInputs {
    pub now: Instant,
    pub power: PowerState,
    pub fans: FanInputs,
    pub thermal: Thermal,
    pub awake: AwakeInputs,
    /// Some network path is up.
    pub online: bool,
    pub storage: StorageInputs,
}

impl AlertInputs {
    /// Nothing happening at `now`: on AC, online, fans automatic.
    pub fn at(now: Instant) -> Self {
        Self {
            now,
            power: PowerState::default(),
            fans: FanInputs::default(),
            thermal: Thermal::Nominal,
            awake: AwakeInputs::default(),
            online: true,
            storage: StorageInputs::default(),
        }
    }
}

/// The kinds of notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlertKind {
    FansHighOnBattery,
    LowBatteryAwake,
    LidSleepOnBattery,
    AwakeEndingSoon,
    ThermalPressure,
    FanOverheat,
    FanHelperLost,
    ForeignFanControl,
    NetworkOffline,
    StorageRed,
}

impl AlertKind {
    pub const ALL: [AlertKind; 10] = [
        AlertKind::FansHighOnBattery,
        AlertKind::LowBatteryAwake,
        AlertKind::LidSleepOnBattery,
        AlertKind::AwakeEndingSoon,
        AlertKind::ThermalPressure,
        AlertKind::FanOverheat,
        AlertKind::FanHelperLost,
        AlertKind::ForeignFanControl,
        AlertKind::NetworkOffline,
        AlertKind::StorageRed,
    ];

    /// A stable id, for replacing and removing the delivered notification.
    pub fn tag(self) -> &'static str {
        match self {
            AlertKind::FansHighOnBattery => "fans-high-battery",
            AlertKind::LowBatteryAwake => "low-battery-awake",
            AlertKind::LidSleepOnBattery => "lid-sleep-battery",
            AlertKind::AwakeEndingSoon => "awake-ending-soon",
            AlertKind::ThermalPressure => "thermal-pressure",
            AlertKind::FanOverheat => "fan-overheat",
            AlertKind::FanHelperLost => "fan-helper-lost",
            AlertKind::ForeignFanControl => "foreign-fan-control",
            AlertKind::NetworkOffline => "network-offline",
            AlertKind::StorageRed => "storage-red",
        }
    }

    /// The kind with this [`AlertKind::tag`], e.g. from a notification's
    /// action callback.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.tag() == tag)
    }

    /// Whether its notification is taken back when the condition clears.
    /// [`AlertKind::LidSleepOnBattery`] is a one-off warning.
    pub fn retracts(self) -> bool {
        self != AlertKind::LidSleepOnBattery
    }
}

/// A notification to post. Posting one replaces any showing with the same
/// [`AlertKind::tag`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    /// Buttons: (action id, label). See the `ACTION_*` constants.
    pub actions: Vec<(&'static str, &'static str)>,
}

/// What [`Alerts::tick`] asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertEvent {
    Post(Alert),
    /// Remove the delivered notification with this kind's tag.
    Retract(AlertKind),
}

/// Which notifications to post, from
/// [`crate::settings::Settings::alert_prefs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertPrefs {
    /// Master switch.
    pub enabled: bool,
    pub fans_high_on_battery: bool,
    pub low_battery_awake: bool,
    pub lid_sleep_on_battery: bool,
    pub awake_ending_soon: bool,
    pub thermal_pressure: bool,
    pub fan_overheat: bool,
    pub fan_helper_lost: bool,
    pub foreign_fan_control: bool,
    pub network_offline: bool,
    pub storage_red: bool,
    /// The app posts speed test results itself; kept here so all switches
    /// are in one place.
    pub speed_test: bool,
    /// [`AlertKind::LowBatteryAwake`] posts at this percentage...
    pub low_battery_pct: u8,
    /// ...and again at this one.
    pub critical_battery_pct: u8,
}

impl Default for AlertPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            fans_high_on_battery: true,
            low_battery_awake: true,
            lid_sleep_on_battery: true,
            awake_ending_soon: false,
            thermal_pressure: true,
            fan_overheat: true,
            fan_helper_lost: true,
            foreign_fan_control: true,
            network_offline: false,
            storage_red: true,
            speed_test: true,
            low_battery_pct: 20,
            critical_battery_pct: 10,
        }
    }
}

impl AlertPrefs {
    /// Whether `kind` may post (the master switch and its own).
    pub fn allows(&self, kind: AlertKind) -> bool {
        self.enabled
            && match kind {
                AlertKind::FansHighOnBattery => self.fans_high_on_battery,
                AlertKind::LowBatteryAwake => self.low_battery_awake,
                AlertKind::LidSleepOnBattery => self.lid_sleep_on_battery,
                AlertKind::AwakeEndingSoon => self.awake_ending_soon,
                AlertKind::ThermalPressure => self.thermal_pressure,
                AlertKind::FanOverheat => self.fan_overheat,
                AlertKind::FanHelperLost => self.fan_helper_lost,
                AlertKind::ForeignFanControl => self.foreign_fan_control,
                AlertKind::NetworkOffline => self.network_offline,
                AlertKind::StorageRed => self.storage_red,
            }
    }

    /// Whether the app should post speed test results.
    pub fn allows_speed_test(&self) -> bool {
        self.enabled && self.speed_test
    }
}

/// What a rule wants this tick.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Post(Alert),
    Retract,
}

fn alert(
    kind: AlertKind,
    title: &str,
    body: String,
    actions: &[(&'static str, &'static str)],
) -> Step {
    Step::Post(Alert {
        kind,
        title: title.into(),
        body,
        actions: actions.to_vec(),
    })
}

/// How long a condition has held.
#[derive(Clone, Debug, Default)]
struct Held {
    since: Option<Instant>,
}

impl Held {
    /// Feed the condition; true once it has held for at least `d`.
    fn at_least(&mut self, cond: bool, now: Instant, d: Duration) -> bool {
        if !cond {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.saturating_duration_since(since) >= d
    }
}

/// Whether `cooldown` has passed since `last`.
fn cooled(last: Option<Instant>, now: Instant, cooldown: Duration) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= cooldown)
}

/// `5 minutes`, `1 minute`, rounding up.
fn minutes(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60).max(1);
    if m == 1 {
        "1 minute".into()
    } else {
        format!("{m} minutes")
    }
}

/// Forced-high fans on battery for [`FANS_HIGH_AFTER`]; repeats every
/// [`FANS_HIGH_REPEAT`]; retracts on AC or once the fans come down.
#[derive(Debug, Default)]
struct FansHigh {
    held: Held,
    /// Set while posted: when to post again.
    next: Option<Instant>,
}

impl FansHigh {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let cond = i.power.on_battery && i.fans.manual && i.fans.forced_high;
        if self.held.at_least(cond, i.now, FANS_HIGH_AFTER) {
            if self.next.is_some_and(|n| i.now < n) {
                return None;
            }
            self.next = Some(i.now + FANS_HIGH_REPEAT);
            let charge = i
                .power
                .percent
                .map(|p| format!(" ({p}%)"))
                .unwrap_or_default();
            return Some(alert(
                AlertKind::FansHighOnBattery,
                "Fans at Full Speed on Battery",
                format!(
                    "Fans are at full speed and you're on battery{charge}. This drains the battery faster."
                ),
                &[
                    (ACTION_FANS_AUTO, "Set to Automatic"),
                    (ACTION_SNOOZE_1H, "Snooze 1 Hour"),
                ],
            ));
        }
        (!cond && self.next.take().is_some()).then_some(Step::Retract)
    }
}

/// On battery with a session running: once at the low threshold, again at
/// the critical one. Re-arms on AC, when charging, or
/// [`LOW_BATTERY_REARM`] points above low.
#[derive(Debug, Default)]
struct LowBattery {
    /// 0 armed, 1 posted at low, 2 posted at critical.
    stage: u8,
    shown: bool,
}

impl LowBattery {
    fn step(&mut self, i: &AlertInputs, prefs: &AlertPrefs) -> Option<Step> {
        let (low, critical) = (
            prefs.low_battery_pct,
            prefs.critical_battery_pct.min(prefs.low_battery_pct),
        );
        let p = i.power.percent;
        // Re-arm on power, above the margin, or when the session ends (a
        // new session at the same low charge should warn again).
        if !i.power.on_battery
            || i.power.charging
            || !i.awake.active
            || p.is_some_and(|p| p > low.saturating_add(LOW_BATTERY_REARM))
        {
            self.stage = 0;
        }
        let base = i.power.on_battery && i.awake.active;
        // A charge that reads as unknown for a tick changes nothing.
        if base && p.is_none() {
            return None;
        }
        let Some(p) = p.filter(|&p| base && p <= low) else {
            return std::mem::take(&mut self.shown).then_some(Step::Retract);
        };
        let (stage, title) = if p <= critical {
            (2, "Battery Very Low")
        } else {
            (1, "Battery Low")
        };
        if self.stage >= stage {
            return None;
        }
        self.stage = stage;
        self.shown = true;
        Some(alert(
            AlertKind::LowBatteryAwake,
            title,
            format!(
                "Battery is at {p}% and a keep-awake session is running. Plug in, or end the session to save power."
            ),
            &[(ACTION_AWAKE_END, "End Session")],
        ))
    }
}

/// The power just switched to battery while a session blocks lid-close
/// sleep, which macOS only honours on AC. Once per switch, at most every
/// [`LID_COOLDOWN`].
#[derive(Debug, Default)]
struct LidSleep {
    was_on_battery: Option<bool>,
    last_post: Option<Instant>,
}

impl LidSleep {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let switched = self.was_on_battery == Some(false) && i.power.on_battery;
        self.was_on_battery = Some(i.power.on_battery);
        if !(switched
            && i.awake.active
            && i.awake.closed_display_sleep_blocked
            && cooled(self.last_post, i.now, LID_COOLDOWN))
        {
            return None;
        }
        self.last_post = Some(i.now);
        Some(alert(
            AlertKind::LidSleepOnBattery,
            "Lid Close Will Sleep on Battery",
            "You're on battery now, so closing the lid will put your Mac to sleep despite the keep-awake session.".into(),
            &[],
        ))
    }
}

/// A timed session longer than [`ENDING_MIN_LENGTH`] with
/// [`ENDING_WARN`] or less left. Once per session; retracted when it ends
/// or is extended.
#[derive(Debug, Default)]
struct AwakeEnding {
    fired: bool,
}

impl AwakeEnding {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let a = &i.awake;
        if self.fired && (!a.active || a.remaining.is_none_or(|r| r > ENDING_WARN)) {
            self.fired = false;
            return Some(Step::Retract);
        }
        let left = a.remaining.filter(|r| !r.is_zero() && *r <= ENDING_WARN)?;
        if self.fired || !a.active || a.length.is_none_or(|l| l <= ENDING_MIN_LENGTH) {
            return None;
        }
        self.fired = true;
        Some(alert(
            AlertKind::AwakeEndingSoon,
            "Keep Awake Ending Soon",
            format!("Your keep-awake session ends in {}.", minutes(left)),
            &[
                (ACTION_AWAKE_EXTEND, "Add 30 Minutes"),
                (ACTION_AWAKE_END, "End Now"),
            ],
        ))
    }
}

/// Serious or critical for [`THERMAL_AFTER`]. Posts when an episode starts
/// (at most every [`THERMAL_COOLDOWN`]) and when it escalates; retracted
/// after [`THERMAL_CLEAR_AFTER`] back at nominal or fair.
#[derive(Debug, Default)]
struct ThermalRule {
    hot: Held,
    cool: Held,
    /// The highest level posted this episode.
    posted: Option<Thermal>,
    last_post: Option<Instant>,
}

impl ThermalRule {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let pressured = i.thermal >= Thermal::Serious;
        let hot = self.hot.at_least(pressured, i.now, THERMAL_AFTER);
        if self.cool.at_least(!pressured, i.now, THERMAL_CLEAR_AFTER) {
            return self.posted.take().map(|_| Step::Retract);
        }
        if !hot || self.posted.is_some_and(|p| i.thermal <= p) {
            return None;
        }
        let escalation = self.posted.is_some();
        if !escalation && !cooled(self.last_post, i.now, THERMAL_COOLDOWN) {
            return None;
        }
        self.posted = Some(i.thermal);
        self.last_post = Some(i.now);
        let body = if i.thermal == Thermal::Critical {
            "macOS reports critical thermal pressure and is slowing your Mac down to cool off."
        } else {
            "macOS reports serious thermal pressure, so your Mac may slow down to cool off."
        };
        Some(alert(
            AlertKind::ThermalPressure,
            "Mac Under Thermal Pressure",
            body.into(),
            &[(ACTION_OPEN_FANS, "Show Fans")],
        ))
    }
}

/// The overheating guard engaged: once per engagement, retracted on
/// release. The guard debounces itself.
#[derive(Debug, Default)]
struct FanOverheatRule {
    shown: bool,
}

impl FanOverheatRule {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        match &i.fans.overheat {
            Some(o) if !self.shown => {
                self.shown = true;
                Some(alert(
                    AlertKind::FanOverheat,
                    "Fans Back on Automatic",
                    format!(
                        "{} reached {:.0}°C, so the fans are on automatic until it cools below {GUARD_RELEASE_C:.0}°C.",
                        o.sensor, o.celsius
                    ),
                    &[],
                ))
            }
            None if self.shown => {
                self.shown = false;
                Some(Step::Retract)
            }
            _ => None,
        }
    }
}

/// The helper was ready with fans manual, then gone for
/// [`HELPER_LOST_AFTER`]. Once per incident, retracted when it's back.
#[derive(Debug, Default)]
struct HelperLost {
    /// Last seen ready with fans manual.
    armed: bool,
    down: Held,
    incident: bool,
}

impl HelperLost {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let ready = i.fans.helper == HelperState::Ready;
        let gone = self.down.at_least(!ready, i.now, HELPER_LOST_AFTER);
        if ready {
            self.armed = i.fans.manual;
            return std::mem::take(&mut self.incident).then_some(Step::Retract);
        }
        if !gone || !self.armed || self.incident {
            return None;
        }
        self.incident = true;
        self.armed = false;
        Some(alert(
            AlertKind::FanHelperLost,
            "Fan Helper Stopped Responding",
            "Utilify lost contact with the fan helper, so the fans are back on automatic.".into(),
            &[(ACTION_OPEN_FANS, "Show Fans")],
        ))
    }
}

/// We had fans manual and another app took them, for [`FOREIGN_AFTER`].
/// Once per incident; retracted when it lets go. Needs manual again to
/// re-arm.
#[derive(Debug, Default)]
struct Foreign {
    /// Fans were manual on the last tick nobody else held them.
    had_manual: bool,
    held: Held,
    incident: bool,
}

impl Foreign {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let long = self.held.at_least(i.fans.foreign, i.now, FOREIGN_AFTER);
        if !i.fans.foreign {
            self.had_manual = i.fans.manual;
            return std::mem::take(&mut self.incident).then_some(Step::Retract);
        }
        if !long || !self.had_manual || self.incident {
            return None;
        }
        self.incident = true;
        self.had_manual = false;
        Some(alert(
            AlertKind::ForeignFanControl,
            "Another App Took the Fans",
            "Another app (such as Macs Fan Control) is controlling the fans, so Utilify's fan settings aren't in effect.".into(),
            &[],
        ))
    }
}

/// Offline for [`OFFLINE_AFTER`] after having been online. Once per
/// outage, at most every [`OFFLINE_COOLDOWN`]; retracted on reconnect.
#[derive(Debug, Default)]
struct Offline {
    seen_online: bool,
    down: Held,
    posted: bool,
    last_post: Option<Instant>,
}

impl Offline {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let long = self.down.at_least(!i.online, i.now, OFFLINE_AFTER);
        if i.online {
            self.seen_online = true;
            return std::mem::take(&mut self.posted).then_some(Step::Retract);
        }
        if !long
            || !self.seen_online
            || self.posted
            || !cooled(self.last_post, i.now, OFFLINE_COOLDOWN)
        {
            return None;
        }
        self.posted = true;
        self.last_post = Some(i.now);
        Some(alert(
            AlertKind::NetworkOffline,
            "You're Offline",
            "The network has been unreachable for over a minute.".into(),
            &[],
        ))
    }
}

/// [`StorageAlarm`]'s crossings, at most every [`STORAGE_COOLDOWN`];
/// retracted once usage is back under the re-arm line.
#[derive(Debug, Default)]
struct Storage {
    alarm: StorageAlarm,
    shown: bool,
    last_post: Option<Instant>,
}

impl Storage {
    fn step(&mut self, i: &AlertInputs) -> Option<Step> {
        let used = i.storage.used?;
        let red = i.storage.red;
        let fired = self.alarm.update(used, red);
        if self.shown && used < red - StorageAlarm::REARM_BELOW {
            self.shown = false;
            return Some(Step::Retract);
        }
        if !fired || !cooled(self.last_post, i.now, STORAGE_COOLDOWN) {
            return None;
        }
        self.shown = true;
        self.last_post = Some(i.now);
        Some(alert(
            AlertKind::StorageRed,
            "Storage Almost Full",
            format!("Your startup disk is {:.0}% full.", used * 100.0),
            &[],
        ))
    }
}

/// The rules engine. Keep one for the app's lifetime and call
/// [`Alerts::tick`] every sample.
#[derive(Debug, Default)]
pub struct Alerts {
    fans_high: FansHigh,
    low_battery: LowBattery,
    lid: LidSleep,
    ending: AwakeEnding,
    thermal: ThermalRule,
    overheat: FanOverheatRule,
    helper: HelperLost,
    foreign: Foreign,
    offline: Offline,
    storage: Storage,
    /// Posted, not yet retracted (only kinds that retract).
    live: BTreeSet<AlertKind>,
    /// Snoozed kinds and until when.
    snoozed: BTreeMap<AlertKind, Instant>,
    /// The latest post held back by a snooze, delivered when it ends if
    /// the condition still holds.
    deferred: BTreeMap<AlertKind, Alert>,
}

impl Alerts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Silence `kind` for [`SNOOZE`] from `now` (the "Snooze 1 Hour"
    /// action). If its condition still holds when the snooze ends, it posts
    /// then.
    pub fn snooze(&mut self, kind: AlertKind, now: Instant) {
        self.snoozed.insert(kind, now + SNOOZE);
    }

    /// Whether `kind` is snoozed at `now`.
    pub fn is_snoozed(&self, kind: AlertKind, now: Instant) -> bool {
        self.snoozed.get(&kind).is_some_and(|&until| now < until)
    }

    /// Run every rule once. Returns posts and retractions, in
    /// [`AlertKind::ALL`] order.
    pub fn tick(&mut self, inputs: &AlertInputs, prefs: &AlertPrefs) -> Vec<AlertEvent> {
        let now = inputs.now;
        let steps = [
            (AlertKind::FansHighOnBattery, self.fans_high.step(inputs)),
            (
                AlertKind::LowBatteryAwake,
                self.low_battery.step(inputs, prefs),
            ),
            (AlertKind::LidSleepOnBattery, self.lid.step(inputs)),
            (AlertKind::AwakeEndingSoon, self.ending.step(inputs)),
            (AlertKind::ThermalPressure, self.thermal.step(inputs)),
            (AlertKind::FanOverheat, self.overheat.step(inputs)),
            (AlertKind::FanHelperLost, self.helper.step(inputs)),
            (AlertKind::ForeignFanControl, self.foreign.step(inputs)),
            (AlertKind::NetworkOffline, self.offline.step(inputs)),
            (AlertKind::StorageRed, self.storage.step(inputs)),
        ];
        let mut out = Vec::new();
        for (kind, step) in steps {
            match step {
                Some(Step::Post(a)) => self.post(a, prefs, now, &mut out),
                Some(Step::Retract) => {
                    self.deferred.remove(&kind);
                    if self.live.remove(&kind) {
                        out.push(AlertEvent::Retract(kind));
                    }
                }
                None => {}
            }
        }
        // Snoozes that ran out deliver what they held back.
        let expired: Vec<AlertKind> = self
            .snoozed
            .iter()
            .filter(|&(_, &until)| now >= until)
            .map(|(&k, _)| k)
            .collect();
        for kind in expired {
            self.snoozed.remove(&kind);
            if let Some(a) = self.deferred.remove(&kind) {
                self.post(a, prefs, now, &mut out);
                // A late delivery restarts the repeat, so the rule's own
                // hourly post doesn't follow it within minutes.
                if kind == AlertKind::FansHighOnBattery && self.fans_high.next.is_some() {
                    self.fans_high.next = Some(now + FANS_HIGH_REPEAT);
                }
            }
        }
        // Turned off while showing: take it back.
        for kind in AlertKind::ALL {
            if !prefs.allows(kind) {
                self.deferred.remove(&kind);
                if self.live.remove(&kind) {
                    out.push(AlertEvent::Retract(kind));
                }
            }
        }
        out
    }

    fn post(&mut self, a: Alert, prefs: &AlertPrefs, now: Instant, out: &mut Vec<AlertEvent>) {
        let kind = a.kind;
        if !prefs.allows(kind) {
            return;
        }
        if self.is_snoozed(kind, now) {
            if kind.retracts() {
                self.deferred.insert(kind, a);
            }
            return;
        }
        if kind.retracts() {
            self.live.insert(kind);
        }
        out.push(AlertEvent::Post(a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);
    const MIN: Duration = Duration::from_secs(60);

    /// Drives an engine through made-up time.
    struct Sim {
        alerts: Alerts,
        prefs: AlertPrefs,
        t0: Instant,
        inputs: AlertInputs,
    }

    impl Sim {
        fn new() -> Self {
            let t0 = Instant::now();
            Self {
                alerts: Alerts::new(),
                prefs: AlertPrefs {
                    // Everything on, so each rule can be tested.
                    awake_ending_soon: true,
                    network_offline: true,
                    ..Default::default()
                },
                t0,
                inputs: AlertInputs::at(t0),
            }
        }

        /// Tick at `t0 + at`.
        fn at(&mut self, at: Duration) -> Vec<AlertEvent> {
            self.inputs.now = self.t0 + at;
            self.alerts.tick(&self.inputs, &self.prefs)
        }

        /// Tick every `step` from `from` to `to` inclusive, collecting
        /// events with their times.
        fn run(
            &mut self,
            from: Duration,
            to: Duration,
            step: Duration,
        ) -> Vec<(Duration, AlertEvent)> {
            let mut out = Vec::new();
            let mut t = from;
            while t <= to {
                out.extend(self.at(t).into_iter().map(|e| (t, e)));
                t += step;
            }
            out
        }

        fn snooze(&mut self, kind: AlertKind, at: Duration) {
            self.alerts.snooze(kind, self.t0 + at);
        }
    }

    fn posts(events: &[AlertEvent], kind: AlertKind) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, AlertEvent::Post(a) if a.kind == kind))
            .count()
    }

    fn post_of(events: &[AlertEvent], kind: AlertKind) -> &Alert {
        events
            .iter()
            .find_map(|e| match e {
                AlertEvent::Post(a) if a.kind == kind => Some(a),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {kind:?} post in {events:?}"))
    }

    fn retracted(events: &[AlertEvent], kind: AlertKind) -> bool {
        events.contains(&AlertEvent::Retract(kind))
    }

    fn timed(events: &[(Duration, AlertEvent)]) -> Vec<AlertEvent> {
        events.iter().map(|(_, e)| e.clone()).collect()
    }

    fn on_battery(percent: u8) -> PowerState {
        PowerState {
            on_battery: true,
            percent: Some(percent),
            has_battery: true,
            ..Default::default()
        }
    }

    fn on_ac() -> PowerState {
        PowerState {
            has_battery: true,
            percent: Some(80),
            ..Default::default()
        }
    }

    fn fans_high() -> FanInputs {
        FanInputs {
            manual: true,
            forced_high: true,
            helper: HelperState::Ready,
            ..Default::default()
        }
    }

    fn session(remaining: Option<Duration>, length: Option<Duration>) -> AwakeInputs {
        AwakeInputs {
            active: true,
            remaining,
            length,
            closed_display_sleep_blocked: false,
        }
    }

    // --- helpers -----------------------------------------------------------

    #[test]
    fn tags_are_stable_and_unique() {
        for k in AlertKind::ALL {
            assert_eq!(AlertKind::from_tag(k.tag()), Some(k));
        }
        assert_eq!(AlertKind::FansHighOnBattery.tag(), "fans-high-battery");
        assert_eq!(AlertKind::from_tag("nope"), None);
    }

    #[test]
    fn forced_high_rule() {
        use utilify_smc::FanInfo;
        let fan = |target: Option<f32>| FanStatus {
            info: FanInfo {
                index: 0,
                current: 0.0,
                min: 1200.0,
                max: 5700.0,
                target: 0.0,
                manual: target.is_some(),
            },
            name: "Fan".into(),
            control: crate::fans::Control::Auto,
            target,
            issue: None,
        };
        // 1200 + 0.8 * 4500 = 4800.
        assert!(!forced_high(&Active::Custom, &[fan(Some(4799.0))]));
        assert!(forced_high(&Active::Custom, &[fan(Some(4800.0))]));
        assert!(!forced_high(&Active::Custom, &[fan(None)]));
        assert!(forced_high(&Active::FullBlast, &[fan(None)]));
        assert!(!forced_high(&Active::Custom, &[]));

        let snap = FanSnapshot {
            fans: vec![fan(Some(5000.0))],
            helper: HelperStatus::Ready,
            ..Default::default()
        };
        let f = FanInputs::from_snapshot(&snap, &Active::Custom);
        assert!(f.manual && f.forced_high);
        assert_eq!(f.helper, HelperState::Ready);
        // Full blast that we don't actually hold isn't forced high.
        let idle = FanSnapshot::default();
        assert!(!FanInputs::from_snapshot(&idle, &Active::FullBlast).forced_high);
    }

    #[test]
    fn helper_states_collapse() {
        assert_eq!(
            HelperState::from_status(&HelperStatus::Ready),
            HelperState::Ready
        );
        assert_eq!(
            HelperState::from_status(&HelperStatus::NotRunning),
            HelperState::Lost
        );
        assert_eq!(
            HelperState::from_status(&HelperStatus::Broken("x".into())),
            HelperState::Lost
        );
        assert_eq!(
            HelperState::from_status(&HelperStatus::NotInstalled),
            HelperState::NotInstalled
        );
        assert_eq!(
            HelperState::from_status(&HelperStatus::Unsupported),
            HelperState::NotInstalled
        );
    }

    #[test]
    fn awake_inputs_from_a_session() {
        use crate::awake::{Activation, Allow, KeepAwake, Mode, Session};
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        assert_eq!(
            AwakeInputs::from_keep_awake(&k, Allow::default(), t0),
            AwakeInputs::default()
        );
        k.start(Session::new(
            Mode::For(60 * MIN),
            Activation::Manual,
            None,
            t0,
            0,
        ));
        let a = AwakeInputs::from_keep_awake(&k, Allow::default(), t0 + 20 * MIN);
        assert!(a.active && a.closed_display_sleep_blocked);
        assert_eq!(a.remaining, Some(40 * MIN));
        assert_eq!(a.length, Some(60 * MIN));
        k.start(Session::new(
            Mode::Indefinite,
            Activation::Manual,
            None,
            t0,
            0,
        ));
        let allow = Allow {
            display_sleep: false,
            closed_display_sleep: true,
        };
        let a = AwakeInputs::from_keep_awake(&k, allow, t0);
        assert!(a.active && !a.closed_display_sleep_blocked);
        assert_eq!((a.remaining, a.length), (None, None));
    }

    #[test]
    fn quiet_inputs_post_nothing() {
        let mut s = Sim::new();
        s.inputs.storage.used = Some(0.3);
        assert!(s.run(Duration::ZERO, 3 * HOUR, 10 * SEC).is_empty());
    }

    // --- a. fans high on battery -----------------------------------------

    #[test]
    fn fans_high_waits_ten_minutes_then_repeats_hourly() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(63);
        s.inputs.fans = fans_high();
        let ev = s.run(Duration::ZERO, 10 * MIN - SEC, SEC);
        assert!(ev.is_empty(), "debounced: {ev:?}");
        let ev = s.at(10 * MIN);
        let a = post_of(&ev, AlertKind::FansHighOnBattery);
        assert_eq!(
            a.body,
            "Fans are at full speed and you're on battery (63%). This drains the battery faster."
        );
        assert_eq!(
            a.actions,
            [
                ("fans-auto", "Set to Automatic"),
                ("snooze-1h", "Snooze 1 Hour")
            ]
        );
        let ev = s.run(10 * MIN + 10 * SEC, 3 * HOUR, 10 * SEC);
        let times: Vec<Duration> = ev.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, [70 * MIN, 130 * MIN]);
        // Back on AC: retracted, once.
        s.inputs.power = on_ac();
        assert_eq!(
            s.at(3 * HOUR + SEC),
            [AlertEvent::Retract(AlertKind::FansHighOnBattery)]
        );
        assert!(s.at(3 * HOUR + 2 * SEC).is_empty());
    }

    #[test]
    fn fans_high_resets_when_the_fans_drop_early() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(50);
        s.inputs.fans = fans_high();
        s.run(Duration::ZERO, 9 * MIN, SEC);
        s.inputs.fans.forced_high = false;
        assert!(
            s.at(9 * MIN + SEC).is_empty(),
            "nothing posted, nothing to retract"
        );
        s.inputs.fans.forced_high = true;
        let ev = timed(&s.run(9 * MIN + 2 * SEC, 19 * MIN, SEC));
        assert!(ev.is_empty(), "the ten minutes start over");
        assert_eq!(
            posts(&s.at(19 * MIN + 2 * SEC), AlertKind::FansHighOnBattery),
            1
        );
        // Fans down: retracted.
        s.inputs.fans.forced_high = false;
        assert!(retracted(&s.at(20 * MIN), AlertKind::FansHighOnBattery));
    }

    #[test]
    fn fans_high_snooze_holds_it_for_an_hour() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(63);
        s.inputs.fans = fans_high();
        s.run(Duration::ZERO, 10 * MIN, SEC);
        // The user hits Snooze a little after it posted.
        s.snooze(AlertKind::FansHighOnBattery, 10 * MIN + 5 * SEC);
        assert!(
            s.alerts
                .is_snoozed(AlertKind::FansHighOnBattery, s.t0 + 30 * MIN)
        );
        // The hourly repeat at 70 min falls inside the snooze...
        let ev = s.run(10 * MIN + SEC, 70 * MIN + 4 * SEC, SEC);
        assert!(ev.is_empty(), "{ev:?}");
        // ...and is delivered the moment it ends.
        s.inputs.power = on_battery(41);
        let ev = s.at(70 * MIN + 5 * SEC);
        assert_eq!(posts(&ev, AlertKind::FansHighOnBattery), 1);
        assert!(
            !s.alerts
                .is_snoozed(AlertKind::FansHighOnBattery, s.t0 + 70 * MIN + 5 * SEC)
        );
    }

    #[test]
    fn late_snooze_delivery_restarts_the_hourly_repeat() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(63);
        s.inputs.fans = fans_high();
        s.run(Duration::ZERO, 10 * MIN, SEC);
        // Snoozed just before the 70 min repeat: it ends at 129 min.
        s.snooze(AlertKind::FansHighOnBattery, 69 * MIN);
        s.run(10 * MIN + SEC, 129 * MIN - SEC, 10 * SEC);
        assert_eq!(posts(&s.at(129 * MIN), AlertKind::FansHighOnBattery), 1);
        // No second post at 130 min, a minute later.
        let ev = timed(&s.run(129 * MIN + SEC, 188 * MIN, MIN));
        assert_eq!(posts(&ev, AlertKind::FansHighOnBattery), 0, "{ev:?}");
        assert_eq!(posts(&s.at(189 * MIN), AlertKind::FansHighOnBattery), 1);
    }

    #[test]
    fn snoozed_post_is_dropped_if_the_condition_clears() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(63);
        s.inputs.fans = fans_high();
        s.run(Duration::ZERO, 10 * MIN, SEC);
        s.snooze(AlertKind::FansHighOnBattery, 10 * MIN);
        s.run(10 * MIN + SEC, 70 * MIN, 10 * SEC);
        s.inputs.power = on_ac();
        // The original post is still showing, so it's retracted...
        assert!(retracted(&s.at(71 * MIN), AlertKind::FansHighOnBattery));
        // ...and the held-back repeat never arrives.
        let ev = timed(&s.run(71 * MIN + SEC, 3 * HOUR, MIN));
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn fans_high_needs_manual_fans() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(63);
        s.inputs.fans = FanInputs {
            manual: false,
            ..fans_high()
        };
        assert!(s.run(Duration::ZERO, HOUR, 10 * SEC).is_empty());
    }

    // --- b. low battery during a session ---------------------------------

    #[test]
    fn low_battery_posts_at_twenty_then_ten() {
        let mut s = Sim::new();
        s.inputs.awake = session(None, None);
        s.inputs.power = on_battery(25);
        assert!(s.at(Duration::ZERO).is_empty());
        s.inputs.power = on_battery(20);
        let ev = s.at(MIN);
        let a = post_of(&ev, AlertKind::LowBatteryAwake);
        assert_eq!(a.title, "Battery Low");
        assert!(a.body.starts_with("Battery is at 20%"), "{}", a.body);
        assert_eq!(a.actions, [("awake-end", "End Session")]);
        for (i, p) in (11..20).rev().enumerate() {
            s.inputs.power = on_battery(p);
            assert!(s.at(2 * MIN + i as u32 * MIN).is_empty(), "once at {p}%");
        }
        s.inputs.power = on_battery(10);
        assert_eq!(
            post_of(&s.at(20 * MIN), AlertKind::LowBatteryAwake).title,
            "Battery Very Low"
        );
        s.inputs.power = on_battery(5);
        assert!(s.at(21 * MIN).is_empty());
    }

    #[test]
    fn low_battery_rearms_on_charge_or_above_25() {
        let mut s = Sim::new();
        s.inputs.awake = session(None, None);
        s.inputs.power = on_battery(19);
        assert_eq!(posts(&s.at(Duration::ZERO), AlertKind::LowBatteryAwake), 1);
        // Plugged in: retracted and re-armed.
        s.inputs.power = PowerState {
            charging: true,
            ..on_ac()
        };
        assert!(retracted(&s.at(MIN), AlertKind::LowBatteryAwake));
        s.inputs.power = on_battery(19);
        assert_eq!(posts(&s.at(2 * MIN), AlertKind::LowBatteryAwake), 1);

        // Bouncing between 20 and 24 on battery doesn't re-arm...
        s.inputs.power = on_battery(24);
        assert!(retracted(&s.at(3 * MIN), AlertKind::LowBatteryAwake));
        s.inputs.power = on_battery(20);
        assert!(s.at(4 * MIN).is_empty());
        // ...26 does.
        s.inputs.power = on_battery(26);
        s.at(5 * MIN);
        s.inputs.power = on_battery(20);
        assert_eq!(posts(&s.at(6 * MIN), AlertKind::LowBatteryAwake), 1);
    }

    #[test]
    fn low_battery_needs_a_session_and_retracts_when_it_ends() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(15);
        assert!(s.at(Duration::ZERO).is_empty(), "no session");
        s.inputs.awake = session(None, None);
        assert_eq!(posts(&s.at(MIN), AlertKind::LowBatteryAwake), 1);
        s.inputs.awake = AwakeInputs::default();
        assert!(retracted(&s.at(2 * MIN), AlertKind::LowBatteryAwake));
        // A desktop never posts.
        let mut d = Sim::new();
        d.inputs.awake = session(None, None);
        assert!(d.run(Duration::ZERO, HOUR, MIN).is_empty());
    }

    #[test]
    fn low_battery_warns_again_for_a_new_session() {
        let mut s = Sim::new();
        s.inputs.power = on_battery(18);
        s.inputs.awake = session(None, None);
        assert_eq!(posts(&s.at(Duration::ZERO), AlertKind::LowBatteryAwake), 1);
        s.inputs.awake = AwakeInputs::default();
        assert!(retracted(&s.at(MIN), AlertKind::LowBatteryAwake));
        s.inputs.power = on_battery(15);
        s.inputs.awake = session(None, None);
        assert_eq!(posts(&s.at(2 * MIN), AlertKind::LowBatteryAwake), 1);
    }

    #[test]
    fn low_battery_holds_through_an_unknown_charge() {
        let mut s = Sim::new();
        s.inputs.awake = session(None, None);
        s.inputs.power = on_battery(18);
        assert_eq!(posts(&s.at(Duration::ZERO), AlertKind::LowBatteryAwake), 1);
        s.inputs.power = PowerState {
            percent: None,
            ..on_battery(18)
        };
        assert!(s.at(MIN).is_empty(), "no retract on one unknown reading");
        s.inputs.power = on_battery(17);
        assert!(s.at(2 * MIN).is_empty(), "and no repeat after it");
    }

    #[test]
    fn low_battery_uses_the_threshold_from_prefs() {
        let mut s = Sim::new();
        s.prefs.low_battery_pct = 30;
        s.inputs.awake = session(None, None);
        s.inputs.power = on_battery(30);
        assert_eq!(posts(&s.at(Duration::ZERO), AlertKind::LowBatteryAwake), 1);
    }

    // --- c. lid sleep on battery ----------------------------------------

    #[test]
    fn lid_warning_on_each_switch_to_battery() {
        let mut s = Sim::new();
        s.inputs.awake = AwakeInputs {
            closed_display_sleep_blocked: true,
            ..session(None, None)
        };
        s.inputs.power = on_battery(80);
        assert!(
            s.at(Duration::ZERO).is_empty(),
            "starting on battery isn't a switch"
        );
        s.inputs.power = on_ac();
        assert!(s.at(MIN).is_empty());
        s.inputs.power = on_battery(80);
        let ev = s.at(2 * MIN);
        assert_eq!(posts(&ev, AlertKind::LidSleepOnBattery), 1);
        assert!(s.at(3 * MIN).is_empty(), "once per switch");
        // Flapping inside the cooldown stays quiet.
        s.inputs.power = on_ac();
        s.at(4 * MIN);
        s.inputs.power = on_battery(80);
        assert!(s.at(5 * MIN).is_empty());
        // After it, the next switch posts.
        s.inputs.power = on_ac();
        s.at(20 * MIN);
        s.inputs.power = on_battery(80);
        assert_eq!(posts(&s.at(21 * MIN), AlertKind::LidSleepOnBattery), 1);
        // Never retracted, even back on AC.
        s.inputs.power = on_ac();
        assert!(s.at(22 * MIN).is_empty());
    }

    #[test]
    fn lid_warning_needs_closed_display_sleep_blocked() {
        let mut s = Sim::new();
        s.inputs.awake = session(None, None);
        s.inputs.power = on_ac();
        s.at(Duration::ZERO);
        s.inputs.power = on_battery(80);
        assert!(s.at(MIN).is_empty());
    }

    // --- d. awake ending soon -------------------------------------------

    #[test]
    fn ending_soon_once_per_session() {
        let mut s = Sim::new();
        let len = 30 * MIN;
        let at = |s: &mut Sim, t: Duration| {
            s.inputs.awake = session(Some(len.saturating_sub(t)), Some(len));
            s.at(t)
        };
        assert!(at(&mut s, 24 * MIN).is_empty());
        let ev = at(&mut s, 25 * MIN);
        let a = post_of(&ev, AlertKind::AwakeEndingSoon);
        assert_eq!(a.body, "Your keep-awake session ends in 5 minutes.");
        assert_eq!(
            a.actions,
            [("awake-extend", "Add 30 Minutes"), ("awake-end", "End Now")]
        );
        assert!(at(&mut s, 27 * MIN).is_empty());
        // Extended by 30 minutes: retracted and re-armed.
        s.inputs.awake = session(Some(33 * MIN), Some(60 * MIN));
        assert!(retracted(&s.at(27 * MIN + SEC), AlertKind::AwakeEndingSoon));
        s.inputs.awake = session(Some(4 * MIN), Some(60 * MIN));
        assert_eq!(posts(&s.at(56 * MIN), AlertKind::AwakeEndingSoon), 1);
        // The session ends: retracted.
        s.inputs.awake = AwakeInputs::default();
        assert!(retracted(&s.at(60 * MIN), AlertKind::AwakeEndingSoon));
    }

    #[test]
    fn ending_soon_skips_short_and_open_ended_sessions() {
        let mut s = Sim::new();
        s.inputs.awake = session(Some(4 * MIN), Some(15 * MIN));
        assert!(s.at(Duration::ZERO).is_empty());
        s.inputs.awake = session(None, None);
        assert!(s.at(MIN).is_empty());
    }

    #[test]
    fn ending_soon_is_off_by_default() {
        let mut s = Sim::new();
        s.prefs = AlertPrefs::default();
        s.inputs.awake = session(Some(4 * MIN), Some(60 * MIN));
        assert!(s.at(Duration::ZERO).is_empty());
    }

    // --- e. thermal pressure --------------------------------------------

    #[test]
    fn thermal_debounces_escalates_and_clears() {
        let mut s = Sim::new();
        s.inputs.thermal = Thermal::Serious;
        assert!(timed(&s.run(Duration::ZERO, 29 * SEC, SEC)).is_empty());
        let ev = s.at(30 * SEC);
        let a = post_of(&ev, AlertKind::ThermalPressure);
        assert!(a.body.contains("serious"));
        assert_eq!(a.actions, [("open-fans", "Show Fans")]);
        assert!(
            timed(&s.run(31 * SEC, 5 * MIN, SEC)).is_empty(),
            "no repeat"
        );
        // Escalating posts again, at once: it has been hot all along.
        s.inputs.thermal = Thermal::Critical;
        assert_eq!(posts(&s.at(5 * MIN + SEC), AlertKind::ThermalPressure), 1);
        assert!(timed(&s.run(5 * MIN + 2 * SEC, 6 * MIN, SEC)).is_empty());
        // Back to fair: retracted after a minute, not before.
        s.inputs.thermal = Thermal::Fair;
        assert!(timed(&s.run(6 * MIN + SEC, 7 * MIN, SEC)).is_empty());
        assert!(retracted(&s.at(7 * MIN + SEC), AlertKind::ThermalPressure));
    }

    #[test]
    fn thermal_escalation_posts_at_once() {
        let mut s = Sim::new();
        s.inputs.thermal = Thermal::Serious;
        s.run(Duration::ZERO, 30 * SEC, SEC);
        s.inputs.thermal = Thermal::Critical;
        let ev = s.at(31 * SEC);
        assert!(
            post_of(&ev, AlertKind::ThermalPressure)
                .body
                .contains("critical")
        );
        // Dropping back to serious doesn't post.
        s.inputs.thermal = Thermal::Serious;
        assert!(timed(&s.run(32 * SEC, 5 * MIN, SEC)).is_empty());
    }

    #[test]
    fn thermal_cooldown_between_episodes() {
        let mut s = Sim::new();
        s.inputs.thermal = Thermal::Serious;
        s.run(Duration::ZERO, 30 * SEC, SEC);
        s.inputs.thermal = Thermal::Nominal;
        let ev = timed(&s.run(31 * SEC, 2 * MIN, SEC));
        assert!(retracted(&ev, AlertKind::ThermalPressure));
        // A new episode inside 30 minutes waits for the cooldown.
        s.inputs.thermal = Thermal::Serious;
        let ev = s.run(10 * MIN, 30 * MIN + 29 * SEC, SEC);
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!(
            posts(&s.at(30 * MIN + 30 * SEC), AlertKind::ThermalPressure),
            1
        );
    }

    #[test]
    fn thermal_short_spikes_and_dips_are_ignored() {
        let mut s = Sim::new();
        for i in 0..20u32 {
            s.inputs.thermal = if i % 2 == 0 {
                Thermal::Critical
            } else {
                Thermal::Fair
            };
            assert!(s.at(i * 20 * SEC).is_empty());
        }
    }

    // --- f. fan overheat -------------------------------------------------

    #[test]
    fn overheat_posts_once_per_engagement() {
        let mut s = Sim::new();
        s.inputs.fans.overheat = Some(Overheat {
            sensor: "CPU Core Average".into(),
            celsius: 96.4,
        });
        let ev = s.at(Duration::ZERO);
        assert_eq!(
            post_of(&ev, AlertKind::FanOverheat).body,
            "CPU Core Average reached 96°C, so the fans are on automatic until it cools below 80°C."
        );
        s.inputs.fans.overheat.as_mut().unwrap().celsius = 97.0;
        assert!(s.at(SEC).is_empty());
        s.inputs.fans.overheat = None;
        assert_eq!(s.at(2 * SEC), [AlertEvent::Retract(AlertKind::FanOverheat)]);
        s.inputs.fans.overheat = Some(Overheat {
            sensor: "GPU".into(),
            celsius: 95.0,
        });
        assert_eq!(posts(&s.at(3 * SEC), AlertKind::FanOverheat), 1);
    }

    // --- g. fan helper lost ----------------------------------------------

    #[test]
    fn helper_lost_after_ten_seconds_once() {
        let mut s = Sim::new();
        s.inputs.fans = FanInputs {
            manual: true,
            helper: HelperState::Ready,
            ..Default::default()
        };
        s.at(Duration::ZERO);
        // The fans fall back to automatic when the helper goes.
        s.inputs.fans = FanInputs {
            manual: false,
            helper: HelperState::Lost,
            ..Default::default()
        };
        assert!(timed(&s.run(SEC, 10 * SEC, SEC)).is_empty());
        let ev = s.at(11 * SEC);
        assert!(
            post_of(&ev, AlertKind::FanHelperLost)
                .body
                .contains("back on automatic")
        );
        assert!(
            timed(&s.run(12 * SEC, 10 * MIN, 10 * SEC)).is_empty(),
            "once per incident"
        );
        s.inputs.fans.helper = HelperState::Ready;
        assert!(retracted(&s.at(11 * MIN), AlertKind::FanHelperLost));
        // Back but automatic: losing it now is no incident.
        s.inputs.fans.helper = HelperState::Lost;
        assert!(timed(&s.run(12 * MIN, 20 * MIN, 10 * SEC)).is_empty());
    }

    #[test]
    fn helper_blip_or_never_ready_is_quiet() {
        let mut s = Sim::new();
        s.inputs.fans = FanInputs {
            manual: true,
            helper: HelperState::Ready,
            ..Default::default()
        };
        s.at(Duration::ZERO);
        s.inputs.fans.helper = HelperState::Lost;
        s.run(SEC, 9 * SEC, SEC);
        s.inputs.fans.helper = HelperState::Ready;
        assert!(s.at(10 * SEC).is_empty(), "back within ten seconds");
        // Not installed from the start.
        let mut n = Sim::new();
        n.inputs.fans.helper = HelperState::NotInstalled;
        assert!(n.run(Duration::ZERO, 5 * MIN, SEC).is_empty());
    }

    // --- h. foreign fan control ------------------------------------------

    #[test]
    fn foreign_control_once_per_incident() {
        let mut s = Sim::new();
        s.inputs.fans = fans_high();
        s.at(Duration::ZERO);
        s.inputs.fans = FanInputs {
            foreign: true,
            helper: HelperState::Ready,
            ..Default::default()
        };
        assert!(timed(&s.run(SEC, 5 * SEC, SEC)).is_empty());
        assert_eq!(posts(&s.at(6 * SEC), AlertKind::ForeignFanControl), 1);
        assert!(timed(&s.run(7 * SEC, 5 * MIN, SEC)).is_empty());
        s.inputs.fans.foreign = false;
        assert!(retracted(&s.at(6 * MIN), AlertKind::ForeignFanControl));
        // We never went manual again: another takeover is not ours to report.
        s.inputs.fans.foreign = true;
        assert!(timed(&s.run(7 * MIN, 10 * MIN, SEC)).is_empty());
    }

    #[test]
    fn foreign_control_without_our_manual_is_quiet() {
        let mut s = Sim::new();
        s.inputs.fans.foreign = true;
        assert!(s.run(Duration::ZERO, 5 * MIN, SEC).is_empty());
    }

    // --- i. network offline ----------------------------------------------

    #[test]
    fn offline_after_a_minute_once_per_outage() {
        let mut s = Sim::new();
        s.at(Duration::ZERO);
        s.inputs.online = false;
        assert!(timed(&s.run(SEC, 60 * SEC, SEC)).is_empty());
        assert_eq!(posts(&s.at(61 * SEC), AlertKind::NetworkOffline), 1);
        assert!(timed(&s.run(62 * SEC, 30 * MIN, 10 * SEC)).is_empty());
        s.inputs.online = true;
        assert_eq!(
            s.at(31 * MIN),
            [AlertEvent::Retract(AlertKind::NetworkOffline)]
        );
        // The next outage posts again (outside the cooldown).
        s.inputs.online = false;
        let ev = timed(&s.run(32 * MIN, 34 * MIN, SEC));
        assert_eq!(posts(&ev, AlertKind::NetworkOffline), 1);
    }

    #[test]
    fn offline_needs_a_prior_connection_and_respects_the_cooldown() {
        let mut s = Sim::new();
        s.inputs.online = false;
        assert!(
            s.run(Duration::ZERO, 10 * MIN, SEC).is_empty(),
            "never online"
        );
        s.inputs.online = true;
        s.at(11 * MIN);
        s.inputs.online = false;
        let ev = timed(&s.run(12 * MIN, 14 * MIN, SEC));
        assert_eq!(posts(&ev, AlertKind::NetworkOffline), 1);
        s.inputs.online = true;
        s.at(15 * MIN);
        // A second outage at 16 min waits until 10 min after the first post.
        s.inputs.online = false;
        let ev = s.run(16 * MIN, 25 * MIN, SEC);
        let first = ev
            .iter()
            .find(|(_, e)| matches!(e, AlertEvent::Post(_)))
            .unwrap()
            .0;
        assert_eq!(first, 23 * MIN);
    }

    // --- j. storage ------------------------------------------------------

    #[test]
    fn storage_keeps_the_alarm_semantics_plus_a_day_cooldown() {
        let mut s = Sim::new();
        s.inputs.storage = StorageInputs {
            used: Some(0.90),
            red: 0.80,
        };
        assert!(s.at(Duration::ZERO).is_empty(), "first reading never fires");
        s.inputs.storage.used = Some(0.70);
        s.at(MIN);
        s.inputs.storage.used = Some(0.83);
        let ev = s.at(2 * MIN);
        assert_eq!(
            post_of(&ev, AlertKind::StorageRed).body,
            "Your startup disk is 83% full."
        );
        s.inputs.storage.used = Some(0.79);
        assert!(s.at(3 * MIN).is_empty(), "inside the re-arm margin");
        s.inputs.storage.used = Some(0.77);
        assert!(retracted(&s.at(4 * MIN), AlertKind::StorageRed));
        // Crossing again the same day: re-armed but held by the cooldown.
        s.inputs.storage.used = Some(0.81);
        assert!(s.at(5 * MIN).is_empty());
        s.inputs.storage.used = Some(0.70);
        s.at(25 * HOUR);
        s.inputs.storage.used = Some(0.81);
        assert_eq!(posts(&s.at(25 * HOUR + MIN), AlertKind::StorageRed), 1);
        // Unreadable ticks change nothing.
        s.inputs.storage.used = None;
        assert!(s.at(26 * HOUR).is_empty());
    }

    // --- engine ----------------------------------------------------------

    #[test]
    fn disabled_kinds_stay_fed_and_dont_fire_for_old_crossings() {
        let mut s = Sim::new();
        s.prefs.storage_red = false;
        s.inputs.storage = StorageInputs {
            used: Some(0.70),
            red: 0.80,
        };
        s.at(Duration::ZERO);
        s.inputs.storage.used = Some(0.85);
        assert!(s.at(MIN).is_empty(), "disabled");
        // Enabled later, still red: the crossing already happened.
        s.prefs.storage_red = true;
        assert!(s.run(2 * MIN, 10 * MIN, MIN).is_empty());
    }

    #[test]
    fn disabling_a_showing_kind_retracts_it() {
        let mut s = Sim::new();
        s.inputs.fans.overheat = Some(Overheat {
            sensor: "CPU".into(),
            celsius: 99.0,
        });
        assert_eq!(posts(&s.at(Duration::ZERO), AlertKind::FanOverheat), 1);
        s.prefs.fan_overheat = false;
        assert_eq!(s.at(SEC), [AlertEvent::Retract(AlertKind::FanOverheat)]);
        assert!(s.at(2 * SEC).is_empty(), "retracted once");
        // Re-enabled during the same engagement: no repeat.
        s.prefs.fan_overheat = true;
        assert!(s.at(3 * SEC).is_empty());
    }

    #[test]
    fn master_switch_silences_everything() {
        let mut s = Sim::new();
        s.inputs.fans.overheat = Some(Overheat {
            sensor: "CPU".into(),
            celsius: 99.0,
        });
        s.at(Duration::ZERO);
        s.prefs.enabled = false;
        assert_eq!(s.at(SEC), [AlertEvent::Retract(AlertKind::FanOverheat)]);
        s.inputs.online = false;
        s.inputs.thermal = Thermal::Critical;
        assert!(s.run(2 * SEC, 10 * MIN, SEC).is_empty());
        assert!(!s.prefs.allows_speed_test());
    }

    #[test]
    fn minutes_round_up() {
        assert_eq!(minutes(5 * MIN), "5 minutes");
        assert_eq!(minutes(4 * MIN + SEC), "5 minutes");
        assert_eq!(minutes(30 * SEC), "1 minute");
    }
}
