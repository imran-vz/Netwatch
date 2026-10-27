//! Fan control on its own thread: every [`TICK`] it reads the fans (and
//! the sensors, when someone needs them), runs the [`FanController`], and
//! hands a [`FanSnapshot`] to the app. Reading sensors takes about 50 ms
//! and the helper's socket blocks, so none of it belongs on the UI thread.

use super::FanSettings;
use super::client::{HelperClient, HelperPaths};
use super::controller::{FanController, FanStatus, HelperStatus, Overheat, TICK};
use crate::sensors::{Sensor, SensorReader};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Whether this Mac's fans can be set at all (the helper drives the Apple
/// Silicon fan keys only).
pub const CONTROL_SUPPORTED: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// What the UI shows, from the latest tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanSnapshot {
    pub fans: Vec<FanStatus>,
    /// Empty unless sensors were wanted or some fan is manual (the
    /// overheating guard watches them).
    pub sensors: Vec<Sensor>,
    pub helper: HelperStatus,
    /// The helper's last refusal (e.g. the SMC didn't take manual mode).
    pub error: Option<String>,
    /// Another tool is holding the fans.
    pub foreign: bool,
    /// The overheating guard has put the fans back on automatic.
    pub overheat: Option<Overheat>,
    /// Waiting for the system to hand fans over (several seconds, up to
    /// twenty or so right after it took them back).
    pub taking_over: bool,
    /// Set once the first reading is in.
    pub read_at: Option<Instant>,
}

impl FanSnapshot {
    /// The fastest fan's current speed.
    pub fn fastest_rpm(&self) -> Option<f32> {
        self.fans
            .iter()
            .map(|f| f.info.current)
            .max_by(|a, b| a.total_cmp(b))
    }
}

enum Command {
    Settings(FanSettings),
    WantSensors(bool),
    Shutdown(mpsc::Sender<()>),
}

/// The handle; dropping it stops the thread (and the helper puts the fans
/// back when we disconnect), but [`FanService::shutdown`] does it now.
pub struct FanService {
    tx: mpsc::Sender<Command>,
}

impl FanService {
    /// Starts the thread. `on_update` runs on it after every tick.
    pub fn spawn(
        settings: FanSettings,
        paths: HelperPaths,
        on_update: impl Fn(FanSnapshot) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("fans".into())
            .spawn(move || run(settings, paths, rx, on_update));
        if let Err(e) = spawned {
            log::error!("Starting the fan thread failed: {e}");
        }
        Self { tx }
    }

    /// New presets or controls; applied straight away.
    pub fn set_settings(&self, settings: FanSettings) {
        let _ = self.tx.send(Command::Settings(settings));
    }

    /// Read the sensors every tick (the dashboard is showing them, or the
    /// menu bar shows a temperature), not just when a curve needs them.
    pub fn want_sensors(&self, on: bool) {
        let _ = self.tx.send(Command::WantSensors(on));
    }

    /// Fans back to automatic and the thread stopped. Waits up to
    /// `timeout`; true if it finished.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Command::Shutdown(ack)).is_err() {
            return true;
        }
        done.recv_timeout(timeout).is_ok()
    }
}

fn run(
    mut settings: FanSettings,
    paths: HelperPaths,
    rx: mpsc::Receiver<Command>,
    on_update: impl Fn(FanSnapshot),
) {
    let smc = utilify_smc::Smc::open()
        .map_err(|e| log::info!("No SMC, so no fans: {e}"))
        .ok();
    let mut controller = FanController::new(HelperClient::new(paths), CONTROL_SUPPORTED);
    let mut reader: Option<SensorReader> = None;
    let mut want_sensors = false;
    loop {
        let reading = smc.as_ref().map(super::read).unwrap_or_default();
        let sensors = if want_sensors || settings.wants_manual(&reading.fans) {
            reader.get_or_insert_with(SensorReader::new).read()
        } else {
            Vec::new()
        };
        let now = Instant::now();
        let report = controller.tick(&reading, &sensors, &settings, now);
        let last = FanSnapshot {
            fans: report.fans,
            sensors,
            helper: controller.status().clone(),
            error: controller.error().map(str::to_string),
            foreign: controller.foreign_control(&reading, now),
            overheat: report.overheat,
            taking_over: controller.taking_over(),
            read_at: Some(now),
        };
        on_update(last.clone());

        match rx.recv_timeout(report.retry_in.unwrap_or(TICK)) {
            Ok(Command::Settings(s)) => {
                settings = s;
                controller.retry_now();
                // Taking fans over blocks the next tick for seconds: say
                // so now rather than show nothing happening.
                let manual = settings
                    .resolve(&reading.fans)
                    .iter()
                    .filter(|c| !c.is_auto())
                    .count();
                if last.helper.is_ready() && manual > controller.held() {
                    on_update(FanSnapshot {
                        taking_over: true,
                        foreign: false,
                        ..last.clone()
                    });
                }
            }
            Ok(Command::WantSensors(on)) => want_sensors = on,
            Ok(Command::Shutdown(ack)) => {
                controller.release();
                let _ = ack.send(());
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                controller.release();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fans::client::tests::FakeHelper;

    #[test]
    fn runs_and_shuts_down() {
        let h = FakeHelper::two_fans();
        let (tx, rx) = mpsc::channel();
        let service = FanService::spawn(FanSettings::default(), h.paths(), move |s| {
            let _ = tx.send(s);
        });
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(first.read_at.is_some());
        // Whatever this Mac's fans are, a snapshot names each one.
        assert!(first.fans.iter().all(|f| !f.name.is_empty()));
        service.want_sensors(true);
        assert!(service.shutdown(Duration::from_secs(5)));
    }
}
