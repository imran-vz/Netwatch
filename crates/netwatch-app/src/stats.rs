//! The app's model: the sampling [`Monitor`], settings, the stored disk
//! history and the speed test. Views observe it; [`spawn_sampler`] advances
//! it on a timer.

use gpui_kit::*;
use netwatch_core::disk_history::{self, DiskHistory};
use netwatch_core::monitor::Monitor;
use netwatch_core::settings::{self, Settings, StorageAlarm};
use netwatch_core::speedtest::{self, SpeedTestResult};
use std::path::PathBuf;
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
        self.speed_test = SpeedTest::Running {
            started: Instant::now(),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(speedtest::run).await;
            let _ = this.update(cx, |s, cx| {
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
