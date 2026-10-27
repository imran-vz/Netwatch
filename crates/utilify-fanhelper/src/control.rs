//! What the helper does with requests: put fans in manual mode at a clamped
//! speed, give them back to the system, and remember which ones it holds so
//! a disconnect, timeout, signal or crash returns exactly those to
//! automatic.
//!
//! The only keys it ever writes are `Ftst`, `F{n}Md` and `F{n}Tg`.

use crate::log;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use utilify_smc::protocol::{BUSY, Fans, Request, Response, VERSION};
use utilify_smc::{Error, FAN_FORCE, Key, SmcIo, fan_count, fan_key};

pub type ClientId = u64;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The least time between two SETs of one fan.
    pub min_write_interval: Duration,
    /// After `Ftst=1`, Apple Silicon's thermal daemon takes several seconds
    /// (seven or so on an M4) to let go of the fans; until it does,
    /// `F{n}Md=1` is refused (SMC error 0x82) or accepted without sticking,
    /// and `F{n}Tg` gets overwritten. Write them and read them back up to
    /// this many times...
    pub mode_retries: u32,
    /// ...this far apart...
    pub mode_retry_delay: Duration,
    /// ...for at most this long per request. Past it the reply is
    /// [`BUSY`] and the fan stays held (and `Ftst` set), so the client's
    /// next SET carries on where this one left off; the handoff can take
    /// twenty seconds when the system has only just taken the fans back.
    pub handoff_timeout: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            min_write_interval: Duration::from_millis(250),
            mode_retries: 150,
            mode_retry_delay: Duration::from_millis(100),
            handoff_timeout: Duration::from_secs(4),
        }
    }
}

/// Why [`Controller::apply`] didn't get there.
enum Failure {
    /// Still being handed over: worth asking again.
    Busy(String),
    Hard(String),
}

/// How far a read-back target may be from what was written (`fpe2` has
/// quarter-rpm steps).
const TARGET_TOLERANCE: f64 = 1.0;

pub struct Controller<S> {
    smc: S,
    fans: u8,
    config: Config,
    /// Fans we put in manual mode, and the client that asked.
    owners: BTreeMap<u8, ClientId>,
    /// Whether we wrote `Ftst=1` (and so must write it back to 0).
    set_force: bool,
    last_set: BTreeMap<u8, Instant>,
    /// Where the held fans are recorded, for recovery after a crash.
    record: Option<PathBuf>,
}

impl<S: SmcIo> Controller<S> {
    /// Also returns to automatic anything a previous run recorded holding.
    pub fn new(smc: S, config: Config, record: Option<PathBuf>) -> Controller<S> {
        let fans = fan_count(&smc).unwrap_or_else(|e| {
            log!("can't read the fan count ({e}); treating this Mac as fanless");
            0
        });
        let mut c = Controller {
            smc,
            fans,
            config,
            owners: BTreeMap::new(),
            set_force: false,
            last_set: BTreeMap::new(),
            record,
        };
        c.recover();
        c
    }

    pub fn fans(&self) -> u8 {
        self.fans
    }

    /// Fans currently held in manual mode.
    pub fn held(&self) -> BTreeSet<u8> {
        self.owners.keys().copied().collect()
    }

    pub fn handle(&mut self, client: ClientId, req: Request, now: Instant) -> Response {
        match req {
            Request::Hello(VERSION) => Response::Hello {
                version: VERSION,
                fans: self.fans,
            },
            Request::Hello(v) => Response::Err(format!("unsupported protocol version {v}")),
            Request::Ping => Response::Ok,
            Request::Set { fan, rpm } => match self.set(client, fan, rpm, now) {
                Ok(applied) => Response::Applied(applied),
                Err(msg) => Response::Err(msg),
            },
            Request::Auto(fans) => match self.auto(fans) {
                Ok(()) => Response::Ok,
                Err(msg) => Response::Err(msg),
            },
        }
    }

    /// A client went away: its fans go back to automatic.
    pub fn release(&mut self, client: ClientId) {
        let fans: Vec<u8> = self
            .owners
            .iter()
            .filter(|&(_, &c)| c == client)
            .map(|(&f, _)| f)
            .collect();
        for fan in fans {
            log!("client {client} gone: fan {fan} back to automatic");
            self.restore(fan);
        }
        self.clear_force_if_unused();
    }

    /// Every held fan back to automatic, and `Ftst` back to 0 if we set it.
    pub fn restore_all(&mut self) {
        let fans: Vec<u8> = self.owners.keys().copied().collect();
        for fan in fans {
            log!("fan {fan} back to automatic");
            self.restore(fan);
        }
        self.clear_force_if_unused();
    }

    fn set(&mut self, client: ClientId, fan: u8, rpm: f32, now: Instant) -> Result<f32, String> {
        if fan >= self.fans {
            return Err(format!("no fan {fan}"));
        }
        let (mode, target) = self.manual_keys(fan)?;
        if let Some(last) = self.last_set.get(&fan) {
            let since = now.saturating_duration_since(*last);
            if since < self.config.min_write_interval {
                let wait = self.config.min_write_interval - since;
                return Err(format!("too fast: retry in {} ms", wait.as_millis().max(1)));
            }
        }
        let range = |s| fan_key(fan, s).and_then(|k| self.smc.read_f64(k).ok());
        let (min, max) = match (range("Mn"), range("Mx")) {
            (Some(min), Some(max)) if min <= max => (min, max),
            _ => return Err(format!("can't read fan {fan}'s speed range")),
        };
        let applied = (rpm as f64).clamp(min, max);
        self.last_set.insert(fan, now);

        // Record the fan before touching it, so a crash part-way still gets
        // it restored at the next start.
        self.owners.insert(fan, client);
        self.save_record();
        match self.apply(mode, target, applied) {
            Ok(()) => {
                log!("client {client}: fan {fan} manual at {applied:.0} rpm");
                Ok(applied as f32)
            }
            // Keep the fan (and Ftst): the client asks again, or lets go.
            Err(Failure::Busy(msg)) => {
                log!("client {client}: fan {fan} to {applied:.0} rpm: still waiting ({msg})");
                Err(format!("{BUSY} {msg}"))
            }
            Err(Failure::Hard(msg)) => {
                log!("client {client}: fan {fan} to {applied:.0} rpm failed: {msg}");
                self.restore(fan);
                self.clear_force_if_unused();
                Err(msg)
            }
        }
    }

    /// The Apple Silicon sequence: `Ftst=1` if it isn't already, `F{n}Md=1`,
    /// `F{n}Tg`, then read both back. While the thermal daemon lets go it
    /// can knock either back, so both are rewritten until they stick.
    fn apply(&mut self, mode: Key, target: Key, rpm: f64) -> Result<(), Failure> {
        let hard = |e| Failure::Hard(describe(e));
        if self.smc.has_key(FAN_FORCE) && self.smc.read_f64(FAN_FORCE).ok() != Some(1.0) {
            self.set_force = true;
            self.save_record();
            self.write(FAN_FORCE, 1.0).map_err(hard)?;
        }
        let is_manual = |c: &Self| c.smc.read_f64(mode).ok() == Some(1.0);
        let on_target = |c: &Self| {
            c.smc
                .read_f64(target)
                .is_ok_and(|v| (v - rpm).abs() <= TARGET_TOLERANCE)
        };
        let deadline = Instant::now() + self.config.handoff_timeout;
        let mut tries = 0;
        loop {
            let manual = is_manual(self);
            if manual && on_target(self) {
                return Ok(());
            }
            if tries > self.config.mode_retries || Instant::now() > deadline {
                return Err(Failure::Busy(if manual {
                    "the SMC didn't take the target speed yet".into()
                } else {
                    "the SMC didn't take manual mode yet".into()
                }));
            }
            let (key, v) = if manual { (target, rpm) } else { (mode, 1.0) };
            // Not privileged won't get better by waiting; anything else
            // (0x82 while the thermal daemon lets go) is worth retrying.
            if let Err(e @ Error::NotPrivileged) = self.write(key, v) {
                return Err(hard(e));
            }
            tries += 1;
            // Manual mode just stuck: write the target straight away.
            if !manual && is_manual(self) {
                continue;
            }
            if !(is_manual(self) && on_target(self)) {
                std::thread::sleep(self.config.mode_retry_delay);
            }
        }
    }

    fn auto(&mut self, fans: Fans) -> Result<(), String> {
        let list: Vec<u8> = match fans {
            Fans::One(fan) if fan >= self.fans => return Err(format!("no fan {fan}")),
            Fans::One(fan) => vec![fan],
            Fans::All => (0..self.fans).collect(),
        };
        let mut errors = Vec::new();
        for fan in list {
            let result = self.manual_keys(fan).and_then(|(mode, _)| {
                // Not manual (0, or 3 where the system stopped the fan):
                // nothing to write.
                if self.smc.read_f64(mode).ok() != Some(1.0) {
                    return Ok(());
                }
                self.write(mode, 0.0).map_err(describe)
            });
            self.owners.remove(&fan);
            match result {
                Ok(()) => log!("fan {fan} automatic"),
                Err(e) => errors.push(format!("fan {fan}: {e}")),
            }
        }
        self.clear_force_if_unused();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// `F{n}Md` and `F{n}Tg`, if this Mac has them. Intel Macs (and fanless
    /// ones) don't, and the helper doesn't drive them.
    fn manual_keys(&self, fan: u8) -> Result<(Key, Key), String> {
        match (fan_key(fan, "Md"), fan_key(fan, "Tg")) {
            (Some(mode), Some(target)) if self.smc.has_key(mode) && self.smc.has_key(target) => {
                Ok((mode, target))
            }
            _ => Err("manual fan control isn't supported on this Mac".into()),
        }
    }

    fn restore(&mut self, fan: u8) {
        self.owners.remove(&fan);
        if let Some(mode) = fan_key(fan, "Md").filter(|&k| self.smc.has_key(k))
            && let Err(e) = self.write(mode, 0.0)
        {
            log!("fan {fan}: can't return to automatic: {e}");
        }
        self.save_record();
    }

    fn clear_force_if_unused(&mut self) {
        if self.set_force && self.owners.is_empty() {
            if let Err(e) = self.write(FAN_FORCE, 0.0) {
                log!("can't clear Ftst: {e}");
            }
            self.set_force = false;
        }
        self.save_record();
    }

    /// The one place the helper writes: refuses every key but the three.
    fn write(&self, key: Key, v: f64) -> Result<(), Error> {
        let allowed = key == FAN_FORCE
            || (0..self.fans)
                .any(|n| Some(key) == fan_key(n, "Md") || Some(key) == fan_key(n, "Tg"));
        if !allowed {
            log!("refusing to write {key}");
            return Err(Error::BadValue(key));
        }
        self.smc.write_f64(key, v)
    }

    // The record: one line per held fan (`fan 0`) and `force` if we set
    // `Ftst`; no file when there's nothing to restore.

    fn save_record(&self) {
        let Some(path) = &self.record else { return };
        if self.owners.is_empty() && !self.set_force {
            let _ = std::fs::remove_file(path);
            return;
        }
        let mut text: String = self.owners.keys().map(|f| format!("fan {f}\n")).collect();
        if self.set_force {
            text.push_str("force\n");
        }
        let tmp = path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)) {
            log!("can't write {}: {e}", path.display());
        }
    }

    fn recover(&mut self) {
        let Some(path) = &self.record else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            match line.strip_prefix("fan ").map(str::parse::<u8>) {
                Some(Ok(fan)) if fan < self.fans => {
                    self.owners.insert(fan, 0);
                }
                _ if line == "force" => self.set_force = true,
                _ => {}
            }
        }
        if !self.owners.is_empty() || self.set_force {
            log!("recovering from an unclean exit");
            self.restore_all();
        }
        self.save_record();
    }
}

fn describe(e: Error) -> String {
    match e {
        Error::NotPrivileged => "permission denied: SMC writes need root".into(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use utilify_smc::fake::FakeSmc;

    const FTST: Key = FAN_FORCE;

    fn key(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    fn fake() -> FakeSmc {
        FakeSmc::apple_silicon(&[(1350.0, 5777.0), (1350.0, 5777.0)])
    }

    fn config() -> Config {
        Config {
            mode_retry_delay: Duration::ZERO,
            ..Config::default()
        }
    }

    fn controller(smc: &FakeSmc) -> Controller<&FakeSmc> {
        Controller::new(smc, config(), None)
    }

    fn set(fan: u8, rpm: f32) -> Request {
        Request::Set { fan, rpm }
    }

    fn written_keys(smc: &FakeSmc) -> Vec<String> {
        smc.writes().iter().map(|(k, _)| k.to_string()).collect()
    }

    #[test]
    fn hello_and_ping() {
        let smc = fake();
        let mut c = controller(&smc);
        let now = Instant::now();
        assert_eq!(
            c.handle(1, Request::Hello(1), now),
            Response::Hello {
                version: 1,
                fans: 2
            }
        );
        assert!(matches!(
            c.handle(1, Request::Hello(2), now),
            Response::Err(_)
        ));
        assert_eq!(c.handle(1, Request::Ping, now), Response::Ok);
        assert!(smc.writes().is_empty());
    }

    #[test]
    fn set_unlocks_then_sets_mode_and_target() {
        let smc = fake();
        let mut c = controller(&smc);
        assert_eq!(
            c.handle(1, set(0, 3000.0), Instant::now()),
            Response::Applied(3000.0)
        );
        assert_eq!(written_keys(&smc), ["Ftst", "F0Md", "F0Tg"]);
        assert_eq!(smc.get_f64(key("F0Md")), Some(1.0));
        assert_eq!(smc.get_f64(key("F0Tg")), Some(3000.0));
        assert_eq!(c.held(), BTreeSet::from([0]));
    }

    #[test]
    fn leaves_ftst_alone_when_already_set() {
        let smc = fake();
        smc.set_f64(FTST, 1.0);
        let mut c = controller(&smc);
        c.handle(1, set(1, 2000.0), Instant::now());
        assert_eq!(written_keys(&smc), ["F1Md", "F1Tg"]);
        c.release(1);
        // Someone else's Ftst stays.
        assert_eq!(smc.get_f64(FTST), Some(1.0));
        assert_eq!(smc.get_f64(key("F1Md")), Some(0.0));
    }

    #[test]
    fn clamps_to_the_fan_range() {
        let smc = fake();
        let mut c = controller(&smc);
        let t = Instant::now();
        assert_eq!(c.handle(1, set(0, 99_999.0), t), Response::Applied(5777.0));
        assert_eq!(c.handle(1, set(1, -5.0), t), Response::Applied(1350.0));
        assert_eq!(smc.get_f64(key("F0Tg")), Some(5777.0));
        assert_eq!(smc.get_f64(key("F1Tg")), Some(1350.0));
    }

    #[test]
    fn rate_limits_each_fan() {
        let smc = fake();
        let mut c = controller(&smc);
        let t = Instant::now();
        assert_eq!(c.handle(1, set(0, 2000.0), t), Response::Applied(2000.0));
        let early = c.handle(1, set(0, 2500.0), t + Duration::from_millis(100));
        assert!(
            matches!(early, Response::Err(ref m) if m.starts_with("too fast")),
            "{early:?}"
        );
        // Another fan isn't held up.
        assert_eq!(
            c.handle(1, set(1, 2500.0), t + Duration::from_millis(100)),
            Response::Applied(2500.0)
        );
        assert_eq!(
            c.handle(1, set(0, 2500.0), t + Duration::from_millis(300)),
            Response::Applied(2500.0)
        );
        assert_eq!(smc.get_f64(key("F0Tg")), Some(2500.0));
    }

    #[test]
    fn release_restores_only_that_clients_fans() {
        let smc = fake();
        let mut c = controller(&smc);
        let t = Instant::now();
        c.handle(1, set(0, 3000.0), t);
        c.handle(2, set(1, 3000.0), t);
        c.release(1);
        assert_eq!(smc.get_f64(key("F0Md")), Some(0.0));
        assert_eq!(smc.get_f64(key("F1Md")), Some(1.0));
        assert_eq!(smc.get_f64(FTST), Some(1.0));
        c.release(2);
        assert_eq!(smc.get_f64(key("F1Md")), Some(0.0));
        // The last fan gone, Ftst goes back too.
        assert_eq!(smc.get_f64(FTST), Some(0.0));
        assert!(c.held().is_empty());
    }

    #[test]
    fn restore_all_and_auto() {
        let smc = fake();
        let mut c = controller(&smc);
        let t = Instant::now();
        c.handle(1, set(0, 3000.0), t);
        c.handle(1, set(1, 3000.0), t);
        assert_eq!(c.handle(1, Request::Auto(Fans::One(0)), t), Response::Ok);
        assert_eq!(smc.get_f64(key("F0Md")), Some(0.0));
        assert_eq!(smc.get_f64(FTST), Some(1.0));
        c.restore_all();
        assert_eq!(smc.get_f64(key("F1Md")), Some(0.0));
        assert_eq!(smc.get_f64(FTST), Some(0.0));
        assert!(matches!(
            c.handle(1, Request::Auto(Fans::One(5)), t),
            Response::Err(_)
        ));
        assert_eq!(c.handle(1, Request::Auto(Fans::All), t), Response::Ok);
    }

    #[test]
    fn reports_err_when_the_smc_ignores_manual_mode() {
        let smc = fake();
        smc.ignore_writes(key("F0Md"));
        let mut c = controller(&smc);
        let r = c.handle(1, set(0, 3000.0), Instant::now());
        assert_eq!(
            r,
            Response::Err("busy: the SMC didn't take manual mode yet".into())
        );
        // Still held, Ftst too, so the next SET carries on...
        assert_eq!(c.held(), BTreeSet::from([0]));
        assert_eq!(smc.get_f64(FTST), Some(1.0));
        // ...until the client lets go.
        c.release(1);
        assert!(c.held().is_empty());
        assert_eq!(smc.get_f64(FTST), Some(0.0));
    }

    #[test]
    fn retries_manual_mode_after_unlocking() {
        let smc = fake();
        smc.fail_writes(key("F0Md"), 3);
        let mut c = controller(&smc);
        assert_eq!(
            c.handle(1, set(0, 3000.0), Instant::now()),
            Response::Applied(3000.0)
        );
        assert_eq!(smc.get_f64(key("F0Md")), Some(1.0));
    }

    #[test]
    fn waits_for_manual_mode_to_stick() {
        // Apple Silicon accepts F{n}Md=1 for a while after Ftst=1 without
        // acting on it.
        let smc = fake();
        smc.ignore_next_writes(key("F0Md"), 10);
        let mut c = controller(&smc);
        assert_eq!(
            c.handle(1, set(0, 3000.0), Instant::now()),
            Response::Applied(3000.0)
        );
        assert_eq!(smc.get_f64(key("F0Md")), Some(1.0));
        assert_eq!(smc.get_f64(FTST), Some(1.0));
    }

    #[test]
    fn waits_for_the_target_to_stick() {
        // ...and keeps overwriting F{n}Tg for a while after F{n}Md sticks.
        let smc = fake();
        smc.ignore_next_writes(key("F0Tg"), 10);
        let mut c = controller(&smc);
        assert_eq!(
            c.handle(1, set(0, 5777.0), Instant::now()),
            Response::Applied(5777.0)
        );
        assert_eq!(smc.get_f64(key("F0Tg")), Some(5777.0));
        assert_eq!(smc.get_f64(FTST), Some(1.0));
    }

    #[test]
    fn a_second_set_carries_on_the_handoff() {
        let smc = fake();
        // Longer than one SET's retries.
        smc.ignore_next_writes(key("F0Md"), 200);
        let mut c = controller(&smc);
        let t = Instant::now();
        assert!(matches!(c.handle(1, set(0, 3000.0), t), Response::Err(m) if m.starts_with(BUSY)));
        assert_eq!(
            c.handle(1, set(0, 3000.0), t + Duration::from_secs(1)),
            Response::Applied(3000.0)
        );
        // Ftst was set once, not cleared and set again.
        let ftst = written_keys(&smc).iter().filter(|k| *k == "Ftst").count();
        assert_eq!(ftst, 1);
    }

    #[test]
    fn reports_err_when_the_smc_ignores_the_target() {
        let smc = fake();
        smc.ignore_writes(key("F0Tg"));
        let mut c = controller(&smc);
        let r = c.handle(1, set(0, 3000.0), Instant::now());
        assert_eq!(
            r,
            Response::Err("busy: the SMC didn't take the target speed yet".into())
        );
        assert_eq!(c.handle(1, Request::Auto(Fans::All), Instant::now()), Response::Ok);
        assert!(c.held().is_empty());
        assert_eq!(smc.get_f64(key("F0Md")), Some(0.0));
        assert_eq!(smc.get_f64(FTST), Some(0.0));
    }

    #[test]
    fn reports_permission_errors() {
        let smc = fake();
        smc.deny_writes(true);
        let mut c = controller(&smc);
        match c.handle(1, set(0, 3000.0), Instant::now()) {
            Response::Err(m) => assert!(m.contains("permission denied"), "{m}"),
            r => panic!("{r:?}"),
        }
        assert!(c.held().is_empty());
        // One Ftst try, no Md retries.
        assert_eq!(written_keys(&smc).first().map(String::as_str), Some("Ftst"));
        assert!(!written_keys(&smc).contains(&"F0Tg".to_string()));
    }

    #[test]
    fn refuses_unknown_fans_and_unsupported_macs() {
        let smc = fake();
        let mut c = controller(&smc);
        assert_eq!(
            c.handle(1, set(2, 3000.0), Instant::now()),
            Response::Err("no fan 2".into())
        );
        // An Intel-style SMC: fans but no F{n}Md.
        let intel = FakeSmc::new();
        intel.insert(utilify_smc::FAN_COUNT, utilify_smc::DataType::UI8, &[1]);
        let mut c = controller(&intel);
        assert!(
            matches!(c.handle(1, set(0, 3000.0), Instant::now()), Response::Err(m) if m.contains("supported"))
        );
        assert!(intel.writes().is_empty());
    }

    #[test]
    fn only_writes_the_three_keys() {
        let smc = fake();
        let mut c = controller(&smc);
        let t = Instant::now();
        c.handle(1, set(0, 3000.0), t);
        c.handle(1, set(1, 6000.0), t);
        c.handle(1, Request::Auto(Fans::All), t);
        c.restore_all();
        assert!(c.write(key("F0Mx"), 9000.0).is_err());
        assert!(c.write(key("F2Md"), 1.0).is_err());
        let allowed = ["Ftst", "F0Md", "F0Tg", "F1Md", "F1Tg"];
        for k in written_keys(&smc) {
            assert!(allowed.contains(&k.as_str()), "wrote {k}");
        }
    }

    #[test]
    fn recovers_recorded_fans_at_startup() {
        let dir = std::env::temp_dir().join(format!("fanhelper-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let record = dir.join("state");
        let smc = fake();
        {
            let mut c = Controller::new(&smc, config(), Some(record.clone()));
            c.handle(1, set(1, 3000.0), Instant::now());
            assert!(std::fs::read_to_string(&record).unwrap().contains("fan 1"));
            // Dropped without restoring, as in a crash.
        }
        assert_eq!(smc.get_f64(key("F1Md")), Some(1.0));
        let c = Controller::new(&smc, config(), Some(record.clone()));
        assert!(c.held().is_empty());
        assert_eq!(smc.get_f64(key("F1Md")), Some(0.0));
        assert_eq!(smc.get_f64(FTST), Some(0.0));
        assert!(!record.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_record_means_no_writes_at_startup() {
        let smc = fake();
        smc.set_f64(key("F0Md"), 1.0);
        let _c = Controller::new(
            &smc,
            config(),
            Some(std::env::temp_dir().join("fanhelper-none")),
        );
        // Another tool's manual fan is left alone.
        assert!(smc.writes().is_empty());
    }
}
