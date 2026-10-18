//! The app's model: the sampling [`Monitor`] plus settings. Views observe it;
//! [`spawn_sampler`] advances it on a timer.

use gpui_kit::*;
use netwatch_core::monitor::Monitor;
use netwatch_core::settings::{self, Settings};
use std::path::PathBuf;
use std::time::Instant;

pub struct Stats {
    pub monitor: Monitor,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
}

impl Stats {
    pub fn new(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        Self {
            monitor: Monitor::default(),
            settings,
            settings_path,
        }
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
                stats.update(cx, |s, cx| {
                    s.monitor.tick(Instant::now());
                    cx.notify();
                });
                on_tick(&stats, cx);
                stats.read(cx).settings.interval()
            });
            cx.background_executor().timer(interval).await;
        }
    })
    .detach();
}
