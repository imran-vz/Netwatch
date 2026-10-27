//! Utilify: menu-bar utilities for macOS. A network and storage monitor
//! with keep awake and fan control (on top of `utilify-core`), and a CyberChef-style data
//! workbench (on top of `utilify-chef`). GPUI via gpui-kit, with the status
//! item from `tray-icon`.

pub mod assets;
pub mod dashboard;
pub mod placement;
pub mod stats;
pub mod system;
pub mod theme;
pub mod tray;
pub mod ui;
pub mod workbench;

use crate::stats::{Stats, StatsGlobal};
use crate::tray::{MenuAction, TrayEvent};
use gpui_kit::*;
use utilify_core::awake::{Activation, Mode};

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!(
        "Starting {} v{}",
        utilify_core::PRODUCT_NAME,
        env!("CARGO_PKG_VERSION")
    );
    utilify_core::migrate_legacy_data();
    let settings_path = utilify_core::settings::default_path();
    let settings = settings_path
        .as_deref()
        .map(utilify_core::settings::load)
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
            workbench::init(cx);

            let stats = cx.new(|cx| {
                let mut s = Stats::new(settings.clone(), settings_path);
                s.start_fans(cx);
                s
            });
            cx.set_global(StatsGlobal(stats.clone()));
            let on_quit = stats.clone();
            cx.on_app_quit(move |cx| {
                on_quit.update(cx, |s, _| s.shutdown());
                async {}
            })
            .detach();

            let (tx, rx) = smol::channel::unbounded();
            if let Err(e) = tray::build(&settings, tx) {
                log::error!("Failed to create the status item: {e}");
                cx.quit();
                return;
            }
            spawn_tray_events(rx, cx);
            if settings.awake_on_launch {
                start_keep_awake(cx, Mode::Indefinite, Activation::AtLaunch);
            }
            // Debug builds: open the dashboard at launch, for checking it
            // without clicking the status item. UTILIFY_DEV_TAB picks the
            // tab (overview, network, storage, awake, fans, settings) and
            // UTILIFY_DEV_SPEED_TEST starts a speed test; UTILIFY_DEV_AWAKE
            // a 30-minute keep-awake session; UTILIFY_DEV_PIN
            // keeps it open when focus moves away.
            // UTILIFY_DEV_QUIT_AFTER=<secs> quits through the normal path
            // (fans back to automatic and all), for scripted checks.
            #[cfg(debug_assertions)]
            if let Some(secs) = std::env::var("UTILIFY_DEV_QUIT_AFTER")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
            {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(secs))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
            #[cfg(debug_assertions)]
            if std::env::var_os("UTILIFY_DEV_OPEN_WORKBENCH").is_some() {
                workbench::open(cx);
            }
            #[cfg(debug_assertions)]
            if std::env::var_os("UTILIFY_DEV_OPEN_DASHBOARD").is_some() {
                if let Some(tab) = std::env::var("UTILIFY_DEV_TAB")
                    .ok()
                    .and_then(|t| dashboard::Tab::parse(&t))
                {
                    dashboard::set_tab(tab, cx);
                }
                if std::env::var_os("UTILIFY_DEV_SPEED_TEST").is_some() {
                    stats.update(cx, |s, cx| s.start_speed_test(cx));
                }
                if std::env::var_os("UTILIFY_DEV_AWAKE").is_some() {
                    start_keep_awake(
                        cx,
                        Mode::For(std::time::Duration::from_secs(30 * 60)),
                        Activation::Manual,
                    );
                }
                dashboard::open(cx);
            }
            stats::spawn_sampler(
                stats,
                |stats, cx| {
                    // Sensors every tick only while something shows them.
                    let show = dashboard::is_open(cx) || stats.read(cx).settings.show_temp;
                    stats.update(cx, |s, _| s.want_fan_sensors(show));
                    tray::update(stats.read(cx));
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
        TrayEvent::Menu(MenuAction::OpenWorkbench) => workbench::open(cx),
        TrayEvent::Menu(MenuAction::Quit) => cx.quit(),
        TrayEvent::Menu(MenuAction::SetInterval(ms)) => {
            update_settings(cx, |s| s.interval_ms = ms);
        }
        TrayEvent::Menu(MenuAction::ToggleRates) => {
            update_settings(cx, |s| s.show_rates = !s.show_rates);
        }
        TrayEvent::Menu(MenuAction::ToggleTemp) => {
            update_settings(cx, |s| s.show_temp = !s.show_temp);
        }
        TrayEvent::Menu(MenuAction::ToggleFanRpm) => {
            update_settings(cx, |s| s.show_fan_rpm = !s.show_fan_rpm);
        }
        TrayEvent::Menu(MenuAction::OpenFansTab) => dashboard::open_on(dashboard::Tab::Fans, cx),
        TrayEvent::Menu(MenuAction::FanPreset(choice)) => {
            let settings = stats::stats(cx).read(cx).fan_settings.clone();
            if let Some(active) = choice.active(&settings) {
                update_fan_settings(cx, |s, _| s.active = active);
            }
        }
        TrayEvent::Menu(MenuAction::OpenAwakeTab) => dashboard::open_on(dashboard::Tab::Awake, cx),
        TrayEvent::Menu(MenuAction::StartAwake(mode)) => {
            start_keep_awake(cx, mode, Activation::Manual)
        }
        TrayEvent::Menu(MenuAction::EndAwake) => stop_keep_awake(cx),
        TrayEvent::Menu(MenuAction::ToggleAllowDisplaySleep) => update_settings(cx, |s| {
            s.awake_allow_display_sleep = !s.awake_allow_display_sleep
        }),
        TrayEvent::Menu(MenuAction::ToggleAllowClosedDisplaySleep) => update_settings(cx, |s| {
            s.awake_allow_closed_display_sleep = !s.awake_allow_closed_display_sleep
        }),
        TrayEvent::Menu(MenuAction::ToggleScreenSaver) => {
            update_settings(cx, |s| s.awake_screen_saver = !s.awake_screen_saver)
        }
        TrayEvent::Menu(MenuAction::ToggleAwakeOnLaunch) => {
            update_settings(cx, |s| s.awake_on_launch = !s.awake_on_launch)
        }
        TrayEvent::Menu(MenuAction::ToggleNotifyEnd) => {
            update_settings(cx, |s| s.awake_notify_end = !s.awake_notify_end)
        }
        TrayEvent::Menu(MenuAction::ToggleLaunchAtLogin) => toggle_launch_at_login(cx),
    }
}

/// Change a setting, persist it, and redraw the tray right away.
pub fn update_settings(cx: &mut App, f: impl FnOnce(&mut utilify_core::settings::Settings)) {
    stats::stats(cx).update(cx, |s, cx| s.update_settings(f, cx));
    redraw_tray(cx);
}

/// Change fan presets or controls, persist and apply them, and redraw the
/// tray right away. `f` gets the fans as last read.
pub fn update_fan_settings(
    cx: &mut App,
    f: impl FnOnce(&mut utilify_core::fans::FanSettings, &[utilify_smc::FanInfo]),
) {
    stats::stats(cx).update(cx, |s, cx| s.update_fan_settings(f, cx));
    redraw_tray(cx);
}

fn redraw_tray(cx: &mut App) {
    let stats = stats::stats(cx);
    tray::update(stats.read(cx));
}

/// Start a keep-awake session (replacing a running one) and redraw the tray
/// right away.
pub fn start_keep_awake(cx: &mut App, mode: Mode, activation: Activation) {
    let subject = match mode {
        Mode::WhileApp(pid) => tray::app_name(pid),
        _ => None,
    };
    stats::stats(cx).update(cx, |s, cx| {
        s.start_keep_awake(mode, activation, subject, cx)
    });
    redraw_tray(cx);
}

pub fn stop_keep_awake(cx: &mut App) {
    stats::stats(cx).update(cx, |s, cx| s.stop_keep_awake(cx));
    redraw_tray(cx);
}

/// Flip launch at login. The system holds the state, not our settings.
pub fn toggle_launch_at_login(cx: &mut App) {
    let on = !system::login::enabled().unwrap_or(false);
    if let Err(e) = system::login::set(on) {
        log::warn!("Changing launch at login failed: {e}");
    }
    tray::refresh_system();
    redraw_tray(cx);
}
