//! The app's model: the sampling [`Monitor`], settings, the stored disk
//! history and the speed test. Views observe it; [`spawn_sampler`] advances
//! it on a timer.

use gpui_kit::*;
use netwatch_core::disk_history::{self, DiskHistory};
use netwatch_core::monitor::Monitor;
use netwatch_core::settings::{self, Settings, StorageAlarm};
use netwatch_core::speedtest::{self, SpeedTestResult};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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
}

impl Stats {
    pub fn new(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        let disk_history_path = disk_history::default_path();
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
        }
    }

    /// Sample the OS counters, and store disk usage when a sample is due.
    /// Returns a notification to post when storage just crossed into red.
    pub fn tick(&mut self, now: Instant) -> Option<SystemNotification> {
        self.monitor.tick(now);
        let alert = self.storage_alert();
        if let Some(disk) = self.monitor.disk()
            && self.disk_history.record(disk_history::unix_now(), disk)
            && let Some(path) = &self.disk_history_path
            && let Err(e) = disk_history::save(path, &self.disk_history)
        {
            log::warn!("Saving disk history failed: {e}");
        }
        alert
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
                netwatch_core::format::percent(used),
                netwatch_core::format::bytes(disk.available)
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

    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        f(&mut self.settings);
        if let Some(path) = &self.settings_path
            && let Err(e) = settings::save(path, &self.settings)
        {
            log::warn!("Saving settings failed: {e}");
        }
        cx.notify();
    }
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
                let alert = stats.update(cx, |s, cx| {
                    let alert = s.tick(Instant::now());
                    cx.notify();
                    alert
                });
                if let Some(alert) = alert {
                    log::info!("Storage crossed into red; notifying");
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
