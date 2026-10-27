//! Fan control, like Macs Fan Control: each fan runs automatically (the
//! system's own curve), at a constant speed, or on a curve following a
//! temperature sensor. Presets are named sets of those per-fan controls:
//! the built-in Automatic and Full blast, and the user's own.
//!
//! Reading fans works for any user; setting them goes through the root
//! helper (`utilify-fanhelper`), which [`client`] talks to and
//! [`controller`] drives. [`service`] runs both on a background thread.
//! Presets, the active one and the per-fan controls are kept in
//! `<data dir>/dev.utilify.app/fans.json`.

pub mod client;
pub mod controller;
pub mod service;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use utilify_smc::FanInfo;

/// How one fan is driven.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    /// The system's own fan curve.
    Auto,
    /// A fixed speed in rpm, clamped to the fan's range.
    Constant(f32),
    /// Linear from the fan's minimum at `from_c` to its maximum at `to_c`,
    /// following the sensor with this [`crate::sensors::Sensor::id`].
    SensorCurve {
        sensor_id: String,
        from_c: f32,
        to_c: f32,
    },
}

impl Control {
    pub fn is_auto(&self) -> bool {
        matches!(self, Control::Auto)
    }

    /// What the menu and the fan table say about it: `Auto`, `3000 rpm`,
    /// `CPU Core Average 40–80°C`. `sensor_name` looks up a sensor's name.
    pub fn describe(&self, sensor_name: impl Fn(&str) -> Option<String>) -> String {
        match self {
            Control::Auto => "Auto".into(),
            Control::Constant(rpm) => format!("{rpm:.0} rpm"),
            Control::SensorCurve {
                sensor_id,
                from_c,
                to_c,
            } => format!(
                "{} {from_c:.0}–{to_c:.0}°C",
                sensor_name(sensor_id).unwrap_or_else(|| sensor_id.clone())
            ),
        }
    }
}

/// Starting points for a new sensor-based control.
pub const DEFAULT_FROM_C: f32 = 50.0;
pub const DEFAULT_TO_C: f32 = 85.0;

/// The fan's rpm on a curve: its minimum at or below `from_c`, its maximum
/// at or above `to_c`, linear between. A curve with `to_c <= from_c` is a
/// step at `from_c`.
pub fn curve_rpm(min: f32, max: f32, from_c: f32, to_c: f32, celsius: f32) -> f32 {
    let (lo, hi) = (min.min(max), max.max(min));
    if to_c <= from_c {
        return if celsius >= from_c { hi } else { lo };
    }
    let t = ((celsius - from_c) / (to_c - from_c)).clamp(0.0, 1.0);
    lo + (hi - lo) * t
}

/// A curve only lowers the fan after the temperature has dropped this far
/// below the reading that set the current speed, so it doesn't hunt.
pub const HYSTERESIS_C: f32 = 2.0;
/// How fast a curve may move the fan, in rpm per second. Up quickly when
/// things heat up, down gently.
pub const RAMP_UP_RPM_PER_S: f32 = 1000.0;
pub const RAMP_DOWN_RPM_PER_S: f32 = 250.0;

/// Turns curve readings into a steady target: hysteresis on the way down,
/// and a bounded ramp both ways.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveFollower {
    /// The goal and the temperature that set it.
    goal: Option<(f32, f32)>,
    /// The last target handed out.
    output: Option<f32>,
}

impl CurveFollower {
    /// The target for this reading. `start` seeds the ramp the first time
    /// (the fan's current speed); `dt` is the time since the last step.
    pub fn step(&mut self, raw: f32, celsius: f32, start: f32, dt: Duration) -> f32 {
        let goal = match self.goal {
            // Cooler, but not by enough yet: hold.
            Some((g, t)) if raw < g && celsius > t - HYSTERESIS_C => g,
            _ => {
                self.goal = Some((raw, celsius));
                raw
            }
        };
        let from = self.output.unwrap_or(start);
        let secs = dt.as_secs_f32();
        let out = if goal > from {
            (from + RAMP_UP_RPM_PER_S * secs).min(goal)
        } else {
            (from - RAMP_DOWN_RPM_PER_S * secs).max(goal)
        };
        self.output = Some(out);
        out
    }
}

/// A user's named set of per-fan controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// By fan index; missing fans run automatically.
    pub controls: Vec<Control>,
}

/// Which controls are in effect.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Active {
    /// Every fan automatic.
    #[default]
    Automatic,
    /// Every fan at its maximum.
    FullBlast,
    /// The per-fan controls in [`FanSettings::custom`].
    Custom,
    /// A user preset, by name.
    Preset(String),
}

impl Active {
    pub fn label(&self) -> &str {
        match self {
            Active::Automatic => "Automatic",
            Active::FullBlast => "Full blast",
            Active::Custom => "Custom",
            Active::Preset(name) => name,
        }
    }
}

/// Everything in `fans.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanSettings {
    pub active: Active,
    /// Per-fan controls set in the fan table outside any preset.
    pub custom: Vec<Control>,
    pub presets: Vec<Preset>,
}

/// The longest preset name kept.
pub const MAX_PRESET_NAME: usize = 40;

impl FanSettings {
    /// The control for each of `fans` under the active preset. A preset
    /// that has gone missing falls back to automatic.
    pub fn resolve(&self, fans: &[FanInfo]) -> Vec<Control> {
        let listed = |controls: &[Control]| -> Vec<Control> {
            fans.iter()
                .map(|f| {
                    controls
                        .get(f.index as usize)
                        .cloned()
                        .unwrap_or(Control::Auto)
                })
                .collect()
        };
        match &self.active {
            Active::Automatic => vec![Control::Auto; fans.len()],
            Active::FullBlast => fans.iter().map(|f| Control::Constant(f.max)).collect(),
            Active::Custom => listed(&self.custom),
            Active::Preset(name) => match self.preset(name) {
                Some(p) => listed(&p.controls),
                None => vec![Control::Auto; fans.len()],
            },
        }
    }

    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Whether any fan would leave automatic.
    pub fn wants_manual(&self, fans: &[FanInfo]) -> bool {
        self.resolve(fans).iter().any(|c| !c.is_auto())
    }

    /// The sensors the active controls follow.
    pub fn followed_sensors(&self, fans: &[FanInfo]) -> Vec<String> {
        self.resolve(fans)
            .into_iter()
            .filter_map(|c| match c {
                Control::SensorCurve { sensor_id, .. } => Some(sensor_id),
                _ => None,
            })
            .collect()
    }

    /// Changes one fan's control, from the fan table. Inside a user preset
    /// that edits the preset; otherwise the current controls (Automatic's,
    /// Full blast's or the custom ones) become the custom set, with the
    /// change.
    pub fn set_control(&mut self, fans: &[FanInfo], index: u8, control: Control) {
        let mut controls = self.resolve(fans);
        let Some(pos) = fans.iter().position(|f| f.index == index) else {
            return;
        };
        controls[pos] = control;
        // Stored by fan index.
        let mut by_index =
            vec![Control::Auto; fans.iter().map(|f| f.index as usize + 1).max().unwrap_or(0)];
        for (f, c) in fans.iter().zip(controls) {
            by_index[f.index as usize] = c;
        }
        if let Active::Preset(name) = &self.active
            && let Some(p) = self.presets.iter_mut().find(|p| &p.name == name)
        {
            p.controls = by_index;
            return;
        }
        self.custom = by_index;
        self.active = Active::Custom;
    }

    /// Saves the current controls as a preset named `name` (replacing one
    /// with that name) and makes it active. False for an empty name or one
    /// a built-in uses.
    pub fn save_preset(&mut self, fans: &[FanInfo], name: &str) -> bool {
        let Some(name) = preset_name(name) else {
            return false;
        };
        let mut controls =
            vec![Control::Auto; fans.iter().map(|f| f.index as usize + 1).max().unwrap_or(0)];
        for (f, c) in fans.iter().zip(self.resolve(fans)) {
            controls[f.index as usize] = c;
        }
        match self.presets.iter_mut().find(|p| p.name == name) {
            Some(p) => p.controls = controls,
            None => self.presets.push(Preset {
                name: name.clone(),
                controls,
            }),
        }
        self.active = Active::Preset(name);
        true
    }

    /// False if `to` is empty, built-in or taken, or `from` doesn't exist.
    pub fn rename_preset(&mut self, from: &str, to: &str) -> bool {
        let Some(to) = preset_name(to) else {
            return false;
        };
        if to != from && self.preset(&to).is_some() {
            return false;
        }
        let Some(p) = self.presets.iter_mut().find(|p| p.name == from) else {
            return false;
        };
        p.name = to.clone();
        if self.active == Active::Preset(from.to_string()) {
            self.active = Active::Preset(to);
        }
        true
    }

    /// Deleting the active preset keeps its controls as the custom ones, so
    /// the fans don't change under the user.
    pub fn delete_preset(&mut self, name: &str) {
        let Some(pos) = self.presets.iter().position(|p| p.name == name) else {
            return;
        };
        let p = self.presets.remove(pos);
        if self.active == Active::Preset(name.to_string()) {
            self.custom = p.controls;
            self.active = Active::Custom;
        }
    }
}

/// `name` trimmed and shortened, unless it's empty or a built-in's.
fn preset_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_PRESET_NAME).collect();
    let reserved = [Active::Automatic, Active::FullBlast, Active::Custom]
        .iter()
        .any(|a| a.label().eq_ignore_ascii_case(&name));
    (!name.is_empty() && !reserved).then_some(name)
}

/// "Left side" and "Right side" on two-fan Macs, as Macs Fan Control and
/// Apple's own diagnostics call them; "Fan N" otherwise.
pub fn fan_name(index: u8, count: usize) -> String {
    match (count, index) {
        (1, _) => "Fan".into(),
        (2, 0) => "Left side".into(),
        (2, 1) => "Right side".into(),
        _ => format!("Fan {}", index as u32 + 1),
    }
}

/// Fans as the SMC reports them, plus whether something forces them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanReading {
    pub fans: Vec<FanInfo>,
    /// `Ftst` is 1: some tool has unlocked manual control.
    pub forced: bool,
}

/// Reads the fans and `Ftst`. Empty on fanless Macs and where there's no
/// SMC.
pub fn read(io: &impl utilify_smc::SmcIo) -> FanReading {
    let fans = utilify_smc::read_fans(io).unwrap_or_else(|e| {
        log::debug!("Reading fans failed: {e}");
        Vec::new()
    });
    let forced = io.read_f64(utilify_smc::FAN_FORCE).is_ok_and(|v| v != 0.0);
    FanReading { fans, forced }
}

pub fn default_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join(crate::APP_IDENTIFIER)
            .join("fans.json"),
    )
}

/// Load fan settings; a missing or unreadable file gives the defaults
/// (every fan automatic).
pub fn load(path: &Path) -> FanSettings {
    let mut s: FanSettings = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!(
                "Ignoring unreadable fan settings at {}: {e}",
                path.display()
            );
            FanSettings::default()
        }),
        Err(_) => FanSettings::default(),
    };
    sanitize(&mut s);
    s
}

/// Drops what a hand edit could break: unusable speeds and temperatures,
/// and nameless or duplicate presets.
fn sanitize(s: &mut FanSettings) {
    let fix = |c: &mut Control| match c {
        Control::Constant(rpm) if !rpm.is_finite() || *rpm < 0.0 => *c = Control::Auto,
        Control::SensorCurve { from_c, to_c, .. } if !from_c.is_finite() || !to_c.is_finite() => {
            *c = Control::Auto
        }
        _ => {}
    };
    s.custom.iter_mut().for_each(fix);
    let mut seen = std::collections::HashSet::new();
    s.presets.retain(|p| {
        preset_name(&p.name).as_deref() == Some(p.name.as_str()) && seen.insert(p.name.clone())
    });
    for p in &mut s.presets {
        p.controls.iter_mut().for_each(fix);
    }
    if let Active::Preset(name) = &s.active
        && s.preset(name).is_none()
    {
        s.active = Active::Automatic;
    }
}

/// Save atomically.
pub fn save(path: &Path, settings: &FanSettings) -> std::io::Result<()> {
    crate::write_atomic(path, &serde_json::to_vec_pretty(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fan(index: u8, min: f32, max: f32) -> FanInfo {
        FanInfo {
            index,
            current: min,
            min,
            max,
            target: min,
            manual: false,
        }
    }

    fn two_fans() -> Vec<FanInfo> {
        vec![fan(0, 1200.0, 5700.0), fan(1, 1300.0, 6000.0)]
    }

    fn curve(sensor: &str) -> Control {
        Control::SensorCurve {
            sensor_id: sensor.into(),
            from_c: 40.0,
            to_c: 80.0,
        }
    }

    #[test]
    fn curve_is_linear_between_the_temperatures() {
        assert_eq!(curve_rpm(1000.0, 5000.0, 40.0, 80.0, 20.0), 1000.0);
        assert_eq!(curve_rpm(1000.0, 5000.0, 40.0, 80.0, 40.0), 1000.0);
        assert_eq!(curve_rpm(1000.0, 5000.0, 40.0, 80.0, 60.0), 3000.0);
        assert_eq!(curve_rpm(1000.0, 5000.0, 40.0, 80.0, 70.0), 4000.0);
        assert_eq!(curve_rpm(1000.0, 5000.0, 40.0, 80.0, 95.0), 5000.0);
        // Backwards or empty range: a step.
        assert_eq!(curve_rpm(1000.0, 5000.0, 60.0, 60.0, 59.9), 1000.0);
        assert_eq!(curve_rpm(1000.0, 5000.0, 60.0, 50.0, 60.0), 5000.0);
    }

    #[test]
    fn follower_holds_until_two_degrees_cooler() {
        let mut f = CurveFollower::default();
        let long = Duration::from_secs(60);
        let rpm = |c| curve_rpm(1000.0, 5000.0, 40.0, 80.0, c);
        assert_eq!(f.step(rpm(60.0), 60.0, 1000.0, long), 3000.0);
        // 1.5 °C cooler: stays put.
        assert_eq!(f.step(rpm(58.5), 58.5, 1000.0, long), 3000.0);
        // Warmer again by a little: follows up straight away.
        assert_eq!(f.step(rpm(61.0), 61.0, 1000.0, long), 3100.0);
        // Now relative to 61: 59.5 holds, 59 lowers.
        assert_eq!(f.step(rpm(59.5), 59.5, 1000.0, long), 3100.0);
        assert_eq!(f.step(rpm(59.0), 59.0, 1000.0, long), 2900.0);
        // Hovering around a value never oscillates.
        let mut outputs = vec![];
        for c in [59.0, 58.2, 59.0, 58.2, 59.0] {
            outputs.push(f.step(rpm(c), c, 1000.0, long));
        }
        assert!(outputs.iter().all(|&o| o == 2900.0), "{outputs:?}");
    }

    #[test]
    fn follower_ramps_at_a_bounded_rate() {
        let mut f = CurveFollower::default();
        let tick = Duration::from_secs(2);
        // From the fan's current 1000 rpm towards 5000: 2000 per 2 s tick.
        assert_eq!(f.step(5000.0, 90.0, 1000.0, tick), 3000.0);
        assert_eq!(f.step(5000.0, 90.0, 1000.0, tick), 5000.0);
        assert_eq!(f.step(5000.0, 90.0, 1000.0, tick), 5000.0);
        // Down gently: 500 per tick.
        assert_eq!(f.step(1000.0, 30.0, 1000.0, tick), 4500.0);
        assert_eq!(f.step(1000.0, 30.0, 1000.0, tick), 4000.0);
    }

    #[test]
    fn presets_resolve_per_fan() {
        let fans = two_fans();
        let mut s = FanSettings::default();
        assert_eq!(s.resolve(&fans), vec![Control::Auto, Control::Auto]);
        assert!(!s.wants_manual(&fans));
        s.active = Active::FullBlast;
        assert_eq!(
            s.resolve(&fans),
            vec![Control::Constant(5700.0), Control::Constant(6000.0)]
        );
        s.presets.push(Preset {
            name: "Quiet".into(),
            controls: vec![Control::Constant(2000.0)],
        });
        s.active = Active::Preset("Quiet".into());
        // The second fan isn't listed: automatic.
        assert_eq!(
            s.resolve(&fans),
            vec![Control::Constant(2000.0), Control::Auto]
        );
        s.active = Active::Preset("Gone".into());
        assert_eq!(s.resolve(&fans), vec![Control::Auto, Control::Auto]);
        s.active = Active::Custom;
        s.custom = vec![Control::Auto, curve("cpu_core_average")];
        assert_eq!(s.followed_sensors(&fans), vec!["cpu_core_average"]);
        assert!(s.wants_manual(&fans));
    }

    #[test]
    fn editing_a_fan_makes_custom_or_edits_the_preset() {
        let fans = two_fans();
        let mut s = FanSettings {
            active: Active::FullBlast,
            ..Default::default()
        };
        s.set_control(&fans, 1, Control::Auto);
        assert_eq!(s.active, Active::Custom);
        assert_eq!(s.custom, vec![Control::Constant(5700.0), Control::Auto]);

        assert!(s.save_preset(&fans, "  Loud  "));
        assert_eq!(s.active, Active::Preset("Loud".into()));
        s.set_control(&fans, 1, Control::Constant(3000.0));
        assert_eq!(s.active, Active::Preset("Loud".into()));
        assert_eq!(
            s.preset("Loud").unwrap().controls,
            vec![Control::Constant(5700.0), Control::Constant(3000.0)]
        );
        // Unknown fan: ignored.
        s.set_control(&fans, 7, Control::Auto);
        assert_eq!(s.preset("Loud").unwrap().controls.len(), 2);
    }

    #[test]
    fn preset_names_are_checked() {
        let fans = two_fans();
        let mut s = FanSettings::default();
        assert!(!s.save_preset(&fans, "   "));
        assert!(!s.save_preset(&fans, "automatic"));
        assert!(!s.save_preset(&fans, "Full Blast"));
        assert!(s.save_preset(&fans, "A"));
        assert!(s.save_preset(&fans, "B"));
        assert!(!s.rename_preset("A", "B"), "taken");
        assert!(!s.rename_preset("Nope", "C"));
        assert!(s.rename_preset("B", "C"));
        assert_eq!(s.active, Active::Preset("C".into()), "follows the rename");
        let long = "x".repeat(100);
        assert!(s.save_preset(&fans, &long));
        assert_eq!(s.presets.last().unwrap().name.len(), MAX_PRESET_NAME);
    }

    #[test]
    fn deleting_the_active_preset_keeps_its_controls() {
        let fans = two_fans();
        let mut s = FanSettings {
            active: Active::Custom,
            custom: vec![Control::Constant(2500.0)],
            ..Default::default()
        };
        assert!(s.save_preset(&fans, "Mine"));
        s.delete_preset("Mine");
        assert!(s.presets.is_empty());
        assert_eq!(s.active, Active::Custom);
        assert_eq!(
            s.resolve(&fans),
            vec![Control::Constant(2500.0), Control::Auto]
        );
    }

    #[test]
    fn settings_round_trip_and_default_sanely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/fans.json");
        assert_eq!(load(&path), FanSettings::default());
        let s = FanSettings {
            active: Active::Preset("Work".into()),
            custom: vec![Control::Auto, Control::Constant(3200.0)],
            presets: vec![Preset {
                name: "Work".into(),
                controls: vec![curve("gpu_max"), Control::Auto],
            }],
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);

        std::fs::write(&path, r#"{"active": "full_blast"}"#).unwrap();
        assert_eq!(
            load(&path),
            FanSettings {
                active: Active::FullBlast,
                ..Default::default()
            }
        );
        // A missing preset, a duplicate, a reserved name and a broken speed.
        std::fs::write(
            &path,
            r#"{"active": {"preset": "Gone"},
                "custom": [{"constant": -5}],
                "presets": [{"name": "A", "controls": []}, {"name": "A", "controls": []},
                            {"name": "Automatic", "controls": []}]}"#,
        )
        .unwrap();
        let s = load(&path);
        assert_eq!(s.active, Active::Automatic);
        assert_eq!(s.custom, vec![Control::Auto]);
        assert_eq!(s.presets.len(), 1);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), FanSettings::default());
    }

    #[test]
    fn names_and_descriptions() {
        assert_eq!(fan_name(0, 2), "Left side");
        assert_eq!(fan_name(1, 2), "Right side");
        assert_eq!(fan_name(0, 1), "Fan");
        assert_eq!(fan_name(2, 3), "Fan 3");
        let name = |id: &str| (id == "cpu_core_average").then(|| "CPU Core Average".to_string());
        assert_eq!(Control::Auto.describe(name), "Auto");
        assert_eq!(Control::Constant(2999.6).describe(name), "3000 rpm");
        assert_eq!(
            curve("cpu_core_average").describe(name),
            "CPU Core Average 40–80°C"
        );
        assert_eq!(curve("x").describe(name), "x 40–80°C");
    }

    #[test]
    fn reads_fans_and_the_force_flag() {
        use utilify_smc::fake::FakeSmc;
        let smc = FakeSmc::apple_silicon(&[(1000.0, 5000.0), (1000.0, 5000.0)]);
        let r = read(&smc);
        assert_eq!(r.fans.len(), 2);
        assert!(!r.forced);
        smc.set_f64(utilify_smc::FAN_FORCE, 1.0);
        assert!(read(&smc).forced);
        assert_eq!(read(&FakeSmc::new()), FanReading::default());
    }
}
