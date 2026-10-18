//! The menu-bar status item: a live icon (rates, graph, storage bar), a
//! tooltip with the numbers, and a menu with the preferences.
//!
//! Left click toggles the dashboard; right click opens the menu. The native
//! tray is main-thread-only and lives in [`NATIVE`]; its event handlers run
//! outside GPUI, so they forward [`TrayEvent`]s through a channel.

use netwatch_core::format;
use netwatch_core::icon::{IconRenderer, TrayContent};
use netwatch_core::monitor::Monitor;
use netwatch_core::settings::{INTERVALS_MS, Settings};
use std::cell::RefCell;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click on the status item.
    Clicked,
    Menu(MenuAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenDashboard,
    SetInterval(u64),
    ToggleRates,
    Quit,
}

impl MenuAction {
    pub fn id(self) -> String {
        match self {
            MenuAction::OpenDashboard => "dashboard".into(),
            MenuAction::SetInterval(ms) => format!("interval:{ms}"),
            MenuAction::ToggleRates => "show_rates".into(),
            MenuAction::Quit => "quit".into(),
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "dashboard" => MenuAction::OpenDashboard,
            "show_rates" => MenuAction::ToggleRates,
            "quit" => MenuAction::Quit,
            _ => MenuAction::SetInterval(id.strip_prefix("interval:")?.parse().ok()?),
        })
    }
}

/// Hover text for the status item: current rates and free space.
pub fn tooltip(m: &Monitor) -> String {
    let r = m.rates();
    let rates = format!("↓ {}    ↑ {}", format::rate(r.down), format::rate(r.up));
    let disk = match m.disk() {
        Some(d) => format!(
            "Disk: {} free of {} ({} used)",
            format::bytes(d.available),
            format::bytes(d.total),
            format::percent(d.used_fraction())
        ),
        None => "Disk: unavailable".into(),
    };
    format!("{rates}\n{disk}")
}

pub fn interval_label(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{} s", ms as f64 / 1000.0)
    }
}

struct Native {
    tray: tray_icon::TrayIcon,
    intervals: Vec<(u64, CheckMenuItem)>,
    show_rates: CheckMenuItem,
    renderer: IconRenderer,
    last_icon: Option<netwatch_core::icon::Bitmap>,
    last_tooltip: Option<String>,
}

thread_local! {
    static NATIVE: RefCell<Option<Native>> = const { RefCell::new(None) };
}

/// Create the status item. Must run on the main thread.
pub fn build(settings: &Settings, events: smol::channel::Sender<TrayEvent>) -> anyhow::Result<()> {
    let intervals: Vec<(u64, CheckMenuItem)> = INTERVALS_MS
        .iter()
        .map(|&ms| {
            let item = CheckMenuItem::with_id(
                MenuAction::SetInterval(ms).id(),
                interval_label(ms),
                true,
                ms == settings.interval_ms,
                None,
            );
            (ms, item)
        })
        .collect();
    let refresh = Submenu::new("Refresh Every", true);
    for (_, item) in &intervals {
        refresh.append(item)?;
    }
    let show_rates = CheckMenuItem::with_id(
        MenuAction::ToggleRates.id(),
        "Show Speeds in Menu Bar",
        true,
        settings.show_rates,
        None,
    );
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(MenuAction::OpenDashboard.id(), "Open Dashboard", true, None),
        &refresh,
        &show_rates,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MenuAction::Quit.id(), "Quit Netwatch", true, None),
    ])?;

    let renderer = IconRenderer::new([crate::assets::icon_font()]);
    let first = renderer.tray_icon(&TrayContent::default(), 2.0)?;
    let tray = tray_icon::TrayIconBuilder::new()
        .with_icon(tray_icon::Icon::from_rgba(
            first.rgba,
            first.width,
            first.height,
        )?)
        .with_icon_as_template(true)
        .with_tooltip(netwatch_core::PRODUCT_NAME)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let menu_tx = events.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |e: tray_icon::menu::MenuEvent| {
        if let Some(action) = MenuAction::parse(e.id.as_ref()) {
            let _ = menu_tx.try_send(TrayEvent::Menu(action));
        }
    }));
    tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            let _ = events.try_send(TrayEvent::Clicked);
        }
    }));

    NATIVE.with(|n| {
        *n.borrow_mut() = Some(Native {
            tray,
            intervals,
            show_rates,
            renderer,
            last_icon: None,
            last_tooltip: None,
        })
    });
    Ok(())
}

/// Redraw the icon, tooltip and menu checkmarks from the latest sample. Only touches
/// the native tray for parts that changed.
pub fn update(monitor: &Monitor, settings: &Settings) {
    NATIVE.with(|n| {
        let mut n = n.borrow_mut();
        let Some(n) = n.as_mut() else { return };

        let points = monitor.history().tail(netwatch_core::icon::SLOTS);
        let r = monitor.rates();
        let content = TrayContent {
            points: &points,
            rates: settings.show_rates.then_some((r.down, r.up)),
            disk_used: monitor.disk().map(|d| d.used_fraction()),
        };
        match n.renderer.tray_icon(&content, 2.0) {
            Ok(bmp) if n.last_icon.as_ref() != Some(&bmp) => {
                match tray_icon::Icon::from_rgba(bmp.rgba.clone(), bmp.width, bmp.height) {
                    Ok(icon) => {
                        if let Err(e) = n.tray.set_icon_with_as_template(Some(icon), true) {
                            log::warn!("Setting the tray icon failed: {e}");
                        } else {
                            n.last_icon = Some(bmp);
                        }
                    }
                    Err(e) => log::warn!("Bad tray icon bitmap: {e}"),
                }
            }
            Ok(_) => {}
            Err(e) => log::warn!("Rendering the tray icon failed: {e}"),
        }

        let tip = tooltip(monitor);
        if n.last_tooltip.as_ref() != Some(&tip) {
            n.tray.set_tooltip(Some(&tip)).ok();
            n.last_tooltip = Some(tip);
        }

        for (ms, item) in &n.intervals {
            item.set_checked(*ms == settings.interval_ms);
        }
        n.show_rates.set_checked(settings.show_rates);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::disk::DiskUsage;
    use netwatch_core::net::InterfaceCounters;
    use std::time::{Duration, Instant};

    #[test]
    fn menu_ids_round_trip() {
        for a in [
            MenuAction::OpenDashboard,
            MenuAction::SetInterval(500),
            MenuAction::SetInterval(5000),
            MenuAction::ToggleRates,
            MenuAction::Quit,
        ] {
            assert_eq!(MenuAction::parse(&a.id()), Some(a));
        }
        assert_eq!(MenuAction::parse("interval:abc"), None);
        assert_eq!(MenuAction::parse("bogus"), None);
    }

    #[test]
    fn interval_labels() {
        assert_eq!(interval_label(500), "0.5 s");
        assert_eq!(interval_label(2000), "2 s");
    }

    #[test]
    fn tooltip_shows_rates_and_disk() {
        let iface = |rx| InterfaceCounters {
            name: "en0".into(),
            up: true,
            running: true,
            loopback: false,
            rx_bytes: rx,
            tx_bytes: 0,
            baudrate: 1_000_000_000,
        };
        let mut m = Monitor::new(10);
        let t0 = Instant::now();
        let disk = || {
            Some(DiskUsage {
                total: 500_000_000_000,
                available: 200_000_000_000,
            })
        };
        m.tick_with(t0, &[iface(0)], disk);
        m.tick_with(t0 + Duration::from_secs(1), &[iface(12_500_000)], disk);
        assert_eq!(
            tooltip(&m),
            "↓ 12.5 MB/s    ↑ 0 B/s\nDisk: 200 GB free of 500 GB (60% used)"
        );
        assert_eq!(
            tooltip(&Monitor::new(10)),
            "↓ 0 B/s    ↑ 0 B/s\nDisk: unavailable"
        );
    }
}
