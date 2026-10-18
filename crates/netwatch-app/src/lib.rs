//! Netwatch: a menu-bar network and storage monitor. GPUI (via gpui-kit) on
//! top of `netwatch-core`, with the status item from `tray-icon`.

pub mod assets;
pub mod dashboard;
pub mod placement;
pub mod stats;
pub mod theme;
pub mod tray;

use crate::stats::{Stats, StatsGlobal};
use crate::tray::{MenuAction, TrayEvent};
use gpui_kit::*;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!(
        "Starting {} v{}",
        netwatch_core::PRODUCT_NAME,
        env!("CARGO_PKG_VERSION")
    );
    let settings_path = netwatch_core::settings::default_path();
    let settings = settings_path
        .as_deref()
        .map(netwatch_core::settings::load)
        .unwrap_or_default();

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            // Menu-bar only: no Dock icon or app menu, even when run unbundled
            // (the bundle also sets LSUIElement).
            #[cfg(target_os = "macos")]
            cx.set_activation_policy(ActivationPolicy::Accessory);
            gpui_kit::init(cx);
            theme::init(cx);

            let stats = cx.new(|_| Stats::new(settings.clone(), settings_path));
            cx.set_global(StatsGlobal(stats.clone()));

            let (tx, rx) = smol::channel::unbounded();
            if let Err(e) = tray::build(&settings, tx) {
                log::error!("Failed to create the status item: {e}");
                cx.quit();
                return;
            }
            spawn_tray_events(rx, cx);
            // Debug builds: open the dashboard at launch, for checking it
            // without clicking the status item.
            #[cfg(debug_assertions)]
            if std::env::var_os("NETWATCH_DEV_OPEN_DASHBOARD").is_some() {
                dashboard::open(cx);
            }
            stats::spawn_sampler(
                stats,
                |stats, cx| {
                    let s = stats.read(cx);
                    tray::update(&s.monitor, &s.settings);
                },
                cx,
            );
        });
}

fn spawn_tray_events(rx: smol::channel::Receiver<TrayEvent>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Ok(event) = rx.recv().await {
            cx.update(|cx| handle_tray_event(event, cx));
        }
    })
    .detach();
}

pub fn handle_tray_event(event: TrayEvent, cx: &mut App) {
    match event {
        TrayEvent::Clicked => dashboard::toggle(cx),
        TrayEvent::Menu(MenuAction::OpenDashboard) => dashboard::open(cx),
        TrayEvent::Menu(MenuAction::Quit) => cx.quit(),
        TrayEvent::Menu(MenuAction::SetInterval(ms)) => {
            update_settings(cx, |s| s.interval_ms = ms);
        }
        TrayEvent::Menu(MenuAction::ToggleRates) => {
            update_settings(cx, |s| s.show_rates = !s.show_rates);
        }
    }
}

/// Change a setting, persist it, and redraw the tray right away.
fn update_settings(cx: &mut App, f: impl FnOnce(&mut netwatch_core::settings::Settings)) {
    let stats = stats::stats(cx);
    stats.update(cx, |s, cx| s.update_settings(f, cx));
    let s = stats.read(cx);
    tray::update(&s.monitor, &s.settings);
}
