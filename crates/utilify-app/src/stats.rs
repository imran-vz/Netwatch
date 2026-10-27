//! The app's model: the sampling [`Monitor`], settings, the stored disk
//! history, the speed test, keep-awake sessions and fan control. Views
//! observe it; [`spawn_sampler`] advances it on a timer, and the fan thread
//! ([`FanService`]) posts its own snapshots.

use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use utilify_core::awake::{
    self, Activation, Assertions, KeepAwake, Kind, Mode, ScreenSaverGate, Session, Signals,
};
use utilify_core::disk_history::{self, DiskHistory};
use utilify_core::fans::client::HelperPaths;
use utilify_core::fans::service::{FanService, FanSnapshot};
use utilify_core::fans::{self, FanSettings};
use utilify_core::monitor::Monitor;
use utilify_core::settings::{self, Settings, StorageAlarm};
use utilify_core::speedtest::{self, SpeedTestResult};
use utilify_smc::FanInfo;

#[derive(Clone, Debug, Default)]
pub enum SpeedTest {
    #[default]
    Idle,
    Running {
        started: Instant,
    },
    Done {
        result: SpeedTestResult,
        finished: Instant,
    },
    Failed(String),
}

pub struct Stats {
    pub monitor: Monitor,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    pub disk_history: DiskHistory,
    disk_history_path: Option<PathBuf>,
    pub speed_test: SpeedTest,
    /// Set to stop the running test; each run gets its own flag.
    speed_test_cancel: Option<Arc<AtomicBool>>,
    /// What the speed test showed before the current run, restored on cancel.
    speed_test_before: SpeedTest,
    storage_alarm: StorageAlarm,
    pub keep_awake: KeepAwake,
    awake_path: Option<PathBuf>,
    /// Held while a keep-awake session runs.
    assertions: Assertions,
    screen_saver: ScreenSaverGate,
    /// Watched by "While File Is Downloading".
    downloads_dir: Option<PathBuf>,
    /// The latest fan and sensor readings, and the helper's state.
    pub fans: FanSnapshot,
    pub fan_settings: FanSettings,
    fan_settings_path: Option<PathBuf>,
    fan_service: Option<FanService>,
    /// Last sensor demand sent to the fan thread.
    fan_sensors_wanted: bool,
}

impl Stats {
    pub fn new(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        let disk_history_path = disk_history::default_path();
        let awake_path = awake::default_path();
        let fan_settings_path = fans::default_path();
        Self {
            monitor: Monitor::default(),
            settings,
            settings_path,
            disk_history: disk_history_path
                .as_deref()
                .map(disk_history::load)
                .unwrap_or_default(),
            disk_history_path,
            speed_test: SpeedTest::Idle,
            speed_test_cancel: None,
            speed_test_before: SpeedTest::Idle,
            storage_alarm: StorageAlarm::default(),
            keep_awake: KeepAwake::with_total(
                awake_path
                    .as_deref()
                    .map(awake::load_total)
                    .unwrap_or_default(),
            ),
            awake_path,
            assertions: Assertions::default(),
            screen_saver: ScreenSaverGate::default(),
            downloads_dir: awake::downloads_dir(),
            fans: FanSnapshot::default(),
            fan_settings: fan_settings_path
                .as_deref()
                .map(fans::load)
                .unwrap_or_default(),
            fan_settings_path,
            fan_service: None,
            fan_sensors_wanted: false,
        }
    }

    /// Start the fan thread; its snapshots land in [`Stats::fans`].
    pub fn start_fans(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = smol::channel::unbounded();
        self.fan_service = Some(FanService::spawn(
            self.fan_settings.clone(),
            helper_paths(),
            move |snapshot| {
                let _ = tx.try_send(snapshot);
            },
        ));
        cx.spawn(async move |this, cx| {
            while let Ok(mut snapshot) = rx.recv().await {
                // Only the newest matters.
                while let Ok(newer) = rx.try_recv() {
                    snapshot = newer;
                }
                if this
                    .update(cx, |s, cx| {
                        s.fans = snapshot;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// The fans as last read, for edits that depend on them.
    pub fn fan_infos(&self) -> Vec<FanInfo> {
        self.fans.fans.iter().map(|f| f.info).collect()
    }

    /// Change the fan presets or controls, persist them, and apply them now.
    pub fn update_fan_settings(
        &mut self,
        f: impl FnOnce(&mut FanSettings, &[FanInfo]),
        cx: &mut Context<Self>,
    ) {
        let infos = self.fan_infos();
        f(&mut self.fan_settings, &infos);
        if let Some(path) = &self.fan_settings_path
            && let Err(e) = fans::save(path, &self.fan_settings)
        {
            log::warn!("Saving fan settings failed: {e}");
        }
        if let Some(service) = &self.fan_service {
            service.set_settings(self.fan_settings.clone());
        }
        cx.notify();
    }

    /// Have the fan thread read the sensors every tick (for the dashboard
    /// or the menu-bar temperature), or only when a curve needs them.
    pub fn want_fan_sensors(&mut self, on: bool) {
        if on != self.fan_sensors_wanted
            && let Some(service) = &self.fan_service
        {
            service.want_sensors(on);
            self.fan_sensors_wanted = on;
        }
    }

    /// Sample the OS counters, advance the keep-awake session, and store
    /// disk usage when a sample is due. Returns notifications to post: storage
    /// just crossed into red, or a session ended by itself.
    pub fn tick(&mut self, now: Instant) -> Vec<SystemNotification> {
        self.monitor.tick(now);
        let mut alerts = Vec::new();
        let signals = self.awake_signals();
        if self.keep_awake.tick(now, &signals) {
            log::info!("Keep-awake session ended");
            if self.settings.awake_notify_end {
                alerts.push(SystemNotification {
                    tag: "awake-ended".into(),
                    title: "Keep-awake session ended".into(),
                    body: "Your Mac can sleep normally again.".into(),
                    actions: Vec::new(),
                });
            }
        }
        self.sync_awake();
        self.screen_saver_check();
        alerts.extend(self.storage_alert());
        if let Some(disk) = self.monitor.disk()
            && self.disk_history.record(disk_history::unix_now(), disk)
            && let Some(path) = &self.disk_history_path
            && let Err(e) = disk_history::save(path, &self.disk_history)
        {
            log::warn!("Saving disk history failed: {e}");
        }
        alerts
    }

    /// Only what the running session's mode needs is looked up.
    fn awake_signals(&self) -> Signals {
        let mode = self.keep_awake.session().map(|s| s.mode);
        Signals {
            down_rate: self.monitor.rates().down,
            file_downloading: mode == Some(Mode::WhileFileDownloading)
                && self
                    .downloads_dir
                    .as_deref()
                    .is_some_and(awake::downloads_in_progress),
            app_running: matches!(mode, Some(Mode::WhileApp(pid)) if awake::process_alive(pid)),
        }
    }

    /// "Allow screen saver after N min": blocking display sleep also blocks
    /// the screen saver, so start it ourselves once the user is idle that
    /// long.
    fn screen_saver_check(&mut self) {
        let watching = self.keep_awake.is_active()
            && !self.settings.awake_allow_display_sleep
            && self.settings.awake_screen_saver;
        let idle = if watching {
            awake::idle_time()
        } else {
            Duration::ZERO
        };
        if self
            .screen_saver
            .update(idle, self.settings.awake_screen_saver_after())
        {
            log::info!("Idle for {} s; starting the screen saver", idle.as_secs());
            awake::start_screen_saver();
        }
    }

    fn storage_alert(&mut self) -> Option<SystemNotification> {
        let disk = self.monitor.disk()?;
        let (_, danger) = self.settings.storage_thresholds();
        let used = disk.used_fraction();
        // Keep the alarm fed even when notifications are off, so turning them
        // on doesn't fire for a crossing that already happened.
        let crossed = self.storage_alarm.update(used, danger);
        (crossed && self.settings.storage_notify).then(|| SystemNotification {
            tag: "storage-red".into(),
            title: "Storage is almost full".into(),
            body: format!(
                "The startup disk is {} full, {} free.",
                utilify_core::format::percent(used),
                utilify_core::format::bytes(disk.available)
            )
            .into(),
            actions: Vec::new(),
        })
    }

    /// Start a speed test in the background, unless one is running.
    pub fn start_speed_test(&mut self, cx: &mut Context<Self>) {
        if matches!(self.speed_test, SpeedTest::Running { .. }) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.speed_test_cancel = Some(cancel.clone());
        self.speed_test_before = std::mem::replace(
            &mut self.speed_test,
            SpeedTest::Running {
                started: Instant::now(),
            },
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let flag = cancel.clone();
            let result = smol::unblock(move || speedtest::run(&flag)).await;
            let _ = this.update(cx, |s, cx| {
                // A cancelled run already restored the previous state.
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                s.speed_test_cancel = None;
                s.speed_test = match result {
                    Ok(result) => SpeedTest::Done {
                        result,
                        finished: Instant::now(),
                    },
                    Err(e) => {
                        log::warn!("Speed test failed: {e}");
                        SpeedTest::Failed(e.to_string())
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Stop the running test and go back to what was shown before it.
    pub fn cancel_speed_test(&mut self, cx: &mut Context<Self>) {
        let Some(cancel) = self.speed_test_cancel.take() else {
            return;
        };
        cancel.store(true, Ordering::Relaxed);
        self.speed_test = std::mem::take(&mut self.speed_test_before);
        cx.notify();
    }

    /// Start a keep-awake session, replacing a running one. `subject` names
    /// the app for [`Mode::WhileApp`].
    pub fn start_keep_awake(
        &mut self,
        mode: Mode,
        activation: Activation,
        subject: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.keep_awake.start(Session::new(
            mode,
            activation,
            subject,
            Instant::now(),
            awake::local_secs_of_day(),
        ));
        self.sync_awake();
        cx.notify();
    }

    pub fn stop_keep_awake(&mut self, cx: &mut Context<Self>) {
        self.keep_awake.stop(Instant::now());
        self.sync_awake();
        cx.notify();
    }

    /// On quit: end the session so its last stretch is saved, release the
    /// assertion, and put the fans back to automatic.
    pub fn shutdown(&mut self) {
        self.keep_awake.stop(Instant::now());
        self.sync_awake();
        if let Some(service) = self.fan_service.take()
            && !service.shutdown(Duration::from_secs(2))
        {
            // The helper restores them when our connection closes anyway.
            log::warn!("The fan thread didn't stop in time");
        }
    }

    /// Hold the power assertions the session and settings call for, and
    /// save the awake total when due.
    fn sync_awake(&mut self) {
        let want = if self.keep_awake.is_active() {
            Kind::needed(self.settings.awake_allow())
        } else {
            Vec::new()
        };
        self.assertions.sync(&want);
        if self.keep_awake.take_save_due()
            && let Some(path) = &self.awake_path
            && let Err(e) = awake::save_total(path, self.keep_awake.total())
        {
            log::warn!("Saving the awake total failed: {e}");
        }
    }

    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        f(&mut self.settings);
        self.sync_awake();
        if let Some(path) = &self.settings_path
            && let Err(e) = settings::save(path, &self.settings)
        {
            log::warn!("Saving settings failed: {e}");
        }
        cx.notify();
    }
}

/// The helper's socket. Debug builds can point it elsewhere with
/// UTILIFY_DEV_FAN_SOCKET, e.g. at `utilify-fanhelper --fake --no-root-check
/// --socket /tmp/fans.sock`, to try fan control without touching the fans.
fn helper_paths() -> HelperPaths {
    #[cfg(debug_assertions)]
    if let Some(socket) = std::env::var_os("UTILIFY_DEV_FAN_SOCKET") {
        return HelperPaths {
            socket: socket.into(),
            plist: None,
        };
    }
    HelperPaths::default()
}

/// Global handle so tray events can reach the model.
pub struct StatsGlobal(pub Entity<Stats>);
impl Global for StatsGlobal {}

pub fn stats(cx: &App) -> Entity<Stats> {
    cx.global::<StatsGlobal>().0.clone()
}

/// Sample now, then every `settings.interval` (re-read each tick, so changes
/// apply from the next one). `on_tick` runs after each sample.
pub fn spawn_sampler(
    stats: Entity<Stats>,
    on_tick: impl Fn(&Entity<Stats>, &mut App) + 'static,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        loop {
            let interval = cx.update(|cx| {
                let alerts = stats.update(cx, |s, cx| {
                    let alerts = s.tick(Instant::now());
                    cx.notify();
                    alerts
                });
                for alert in alerts {
                    log::info!("Notifying: {}", alert.title);
                    cx.show_system_notification(alert);
                }
                on_tick(&stats, cx);
                stats.read(cx).settings.interval()
            });
            cx.background_executor().timer(interval).await;
        }
    })
    .detach();
}
