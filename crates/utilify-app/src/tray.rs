//! The menu-bar status item: a live icon (rates, graph, storage bar), a
//! tooltip with the numbers, and a menu with the preferences and keep awake.
//!
//! Left click toggles the dashboard; right click opens the menu. The native
//! tray is main-thread-only and lives in [`NATIVE`]; its event handlers run
//! outside GPUI, so they forward [`TrayEvent`]s through a channel.

use crate::system::{self, RunningApp};
use netwatch_core::awake::{self, KeepAwake, Mode};
use netwatch_core::format;
use netwatch_core::icon::{IconRenderer, TrayContent};
use netwatch_core::monitor::Monitor;
use netwatch_core::settings::{INTERVALS_MS, Settings};
use std::cell::RefCell;
use std::time::{Duration, Instant};
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
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
    /// "Other Time/Until…" and "Keep Awake Settings…": the dashboard's
    /// Awake tab.
    OpenAwakeTab,
    SetInterval(u64),
    ToggleRates,
    StartAwake(Mode),
    EndAwake,
    ToggleAllowDisplaySleep,
    ToggleAllowClosedDisplaySleep,
    ToggleScreenSaver,
    ToggleAwakeOnLaunch,
    ToggleNotifyEnd,
    ToggleLaunchAtLogin,
    Quit,
}

impl MenuAction {
    pub fn id(self) -> String {
        match self {
            MenuAction::OpenDashboard => "dashboard".into(),
            MenuAction::OpenAwakeTab => "awake:tab".into(),
            MenuAction::SetInterval(ms) => format!("interval:{ms}"),
            MenuAction::ToggleRates => "show_rates".into(),
            MenuAction::StartAwake(mode) => format!(
                "awake:start:{}",
                match mode {
                    Mode::Indefinite => "indefinite".into(),
                    Mode::For(d) => format!("for:{}", d.as_secs()),
                    Mode::Until(m) => format!("until:{m}"),
                    Mode::WhileApp(pid) => format!("app:{pid}"),
                    Mode::WhileFileDownloading => "file".into(),
                    Mode::WhileNetworkBusy => "network".into(),
                }
            ),
            MenuAction::EndAwake => "awake:end".into(),
            MenuAction::ToggleAllowDisplaySleep => "awake:allow_display".into(),
            MenuAction::ToggleAllowClosedDisplaySleep => "awake:allow_closed".into(),
            MenuAction::ToggleScreenSaver => "awake:screen_saver".into(),
            MenuAction::ToggleAwakeOnLaunch => "awake:on_launch".into(),
            MenuAction::ToggleNotifyEnd => "awake:notify_end".into(),
            MenuAction::ToggleLaunchAtLogin => "login".into(),
            MenuAction::Quit => "quit".into(),
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "dashboard" => MenuAction::OpenDashboard,
            "awake:tab" => MenuAction::OpenAwakeTab,
            "show_rates" => MenuAction::ToggleRates,
            "awake:end" => MenuAction::EndAwake,
            "awake:allow_display" => MenuAction::ToggleAllowDisplaySleep,
            "awake:allow_closed" => MenuAction::ToggleAllowClosedDisplaySleep,
            "awake:screen_saver" => MenuAction::ToggleScreenSaver,
            "awake:on_launch" => MenuAction::ToggleAwakeOnLaunch,
            "awake:notify_end" => MenuAction::ToggleNotifyEnd,
            "login" => MenuAction::ToggleLaunchAtLogin,
            "quit" => MenuAction::Quit,
            _ => {
                if let Some(start) = id.strip_prefix("awake:start:") {
                    let (kind, arg) = start.split_once(':').unwrap_or((start, ""));
                    MenuAction::StartAwake(match kind {
                        "indefinite" => Mode::Indefinite,
                        "for" => Mode::For(Duration::from_secs(arg.parse().ok()?)),
                        "until" => Mode::Until(arg.parse().ok()?),
                        "app" => Mode::WhileApp(arg.parse().ok()?),
                        "file" => Mode::WhileFileDownloading,
                        "network" => Mode::WhileNetworkBusy,
                        _ => return None,
                    })
                } else {
                    MenuAction::SetInterval(id.strip_prefix("interval:")?.parse().ok()?)
                }
            }
        })
    }
}

/// Hover text for the status item: current rates, free space, and the
/// keep-awake session when one runs.
pub fn tooltip(m: &Monitor, awake: Option<&str>) -> String {
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
    match awake {
        Some(a) => format!("{rates}\n{disk}\nKeeping awake: {a}"),
        None => format!("{rates}\n{disk}"),
    }
}

pub fn interval_label(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{} s", ms as f64 / 1000.0)
    }
}

/// The first line under "Keep Awake", like Amphetamine's session details.
pub fn session_line(k: &KeepAwake, now: Instant) -> String {
    let Some(s) = k.session() else {
        return "No Active Session".into();
    };
    match s.mode {
        Mode::Indefinite => "Indeterminate time remaining".into(),
        Mode::For(_) | Mode::Until(_) => {
            let left = s.remaining(now).unwrap_or_default();
            format!(
                "{} remaining",
                format::countdown(left.as_secs_f64().ceil() as u64)
            )
        }
        _ => s.status(now),
    }
}

pub fn screen_saver_label(settings: &Settings) -> String {
    format!(
        "Allow Screen Saver After {} of Inactivity",
        format::span_long(settings.awake_screen_saver_after().as_secs())
    )
}

/// The running-app list and login state are system queries; refresh them
/// this often rather than every tick.
const SYSTEM_REFRESH: Duration = Duration::from_secs(3);

struct Native {
    tray: tray_icon::TrayIcon,
    intervals: Vec<(u64, CheckMenuItem)>,
    show_rates: CheckMenuItem,
    awake_menu: Submenu,
    session_line: MenuItem,
    activation_line: MenuItem,
    activation_shown: bool,
    allow_display: CheckMenuItem,
    allow_closed: CheckMenuItem,
    screen_saver: CheckMenuItem,
    end_session: MenuItem,
    apps_menu: Submenu,
    apps: Vec<RunningApp>,
    on_launch: CheckMenuItem,
    notify_end: CheckMenuItem,
    login: CheckMenuItem,
    system_checked: Option<Instant>,
    renderer: IconRenderer,
    last_icon: Option<netwatch_core::icon::Bitmap>,
    last_tooltip: Option<String>,
    last_awake_title: Option<String>,
}

thread_local! {
    static NATIVE: RefCell<Option<Native>> = const { RefCell::new(None) };
}

const AWAKE_TITLE: &str = "Keep Awake";

fn cmd(code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(Modifiers::SUPER), code))
}

fn item(action: MenuAction, text: &str, accelerator: Option<Accelerator>) -> MenuItem {
    MenuItem::with_id(action.id(), text, true, accelerator)
}

fn check(action: MenuAction, text: &str, on: bool) -> CheckMenuItem {
    CheckMenuItem::with_id(action.id(), text, true, on, None)
}

/// A disabled line of text.
fn label(text: &str) -> MenuItem {
    MenuItem::new(text, false, None)
}

/// Create the status item. Must run on the main thread.
pub fn build(settings: &Settings, events: smol::channel::Sender<TrayEvent>) -> anyhow::Result<()> {
    let intervals: Vec<(u64, CheckMenuItem)> = INTERVALS_MS
        .iter()
        .map(|&ms| {
            let item = check(
                MenuAction::SetInterval(ms),
                &interval_label(ms),
                ms == settings.interval_ms,
            );
            (ms, item)
        })
        .collect();
    let refresh = Submenu::new("Refresh Every", true);
    for (_, item) in &intervals {
        refresh.append(item)?;
    }
    let show_rates = check(
        MenuAction::ToggleRates,
        "Show Speeds in Menu Bar",
        settings.show_rates,
    );

    // Keep Awake, laid out like Amphetamine's menu: the current session and
    // its options, then the ways to start one, then quick settings.
    let session_line = label("No Active Session");
    let activation_line = label("");
    let allow_display = check(
        MenuAction::ToggleAllowDisplaySleep,
        "Allow Display Sleep",
        settings.awake_allow_display_sleep,
    );
    let allow_closed = check(
        MenuAction::ToggleAllowClosedDisplaySleep,
        "Allow System Sleep When Display Is Closed",
        settings.awake_allow_closed_display_sleep,
    );
    let screen_saver = check(
        MenuAction::ToggleScreenSaver,
        &screen_saver_label(settings),
        settings.awake_screen_saver,
    );
    let end_session = item(MenuAction::EndAwake, "End Current Session", cmd(Code::KeyX));
    end_session.set_enabled(false);

    let minutes = Submenu::new("Minutes", true);
    for m in awake::MINUTES {
        let mode = Mode::For(Duration::from_secs(m * 60));
        minutes.append(&item(MenuAction::StartAwake(mode), &mode.label(), None))?;
    }
    let hours = Submenu::new("Hours", true);
    for h in awake::HOURS {
        let mode = Mode::For(Duration::from_secs(h * 3600));
        hours.append(&item(MenuAction::StartAwake(mode), &mode.label(), None))?;
    }
    let apps_menu = Submenu::new("While App Is Running", true);

    let on_launch = check(
        MenuAction::ToggleAwakeOnLaunch,
        "Start Session When Netwatch Launches",
        settings.awake_on_launch,
    );
    let notify_end = check(
        MenuAction::ToggleNotifyEnd,
        "Notify When Session Ends",
        settings.awake_notify_end,
    );
    let login = check(
        MenuAction::ToggleLaunchAtLogin,
        "Launch Netwatch at Login",
        false,
    );
    let quick = Submenu::new("Quick Settings", true);
    quick.append_items(&[&on_launch, &notify_end, &login])?;

    let awake_menu = Submenu::new(AWAKE_TITLE, true);
    awake_menu.append_items(&[
        &session_line,
        &PredefinedMenuItem::separator(),
        &allow_display,
        &allow_closed,
        &screen_saver,
        &end_session,
        &PredefinedMenuItem::separator(),
        &label("Start New Session:"),
        &item(
            MenuAction::StartAwake(Mode::Indefinite),
            "Indefinitely",
            cmd(Code::KeyI),
        ),
        &minutes,
        &hours,
        &item(MenuAction::OpenAwakeTab, "Other Time/Until…", None),
        &apps_menu,
        &item(
            MenuAction::StartAwake(Mode::WhileFileDownloading),
            "While File Is Downloading",
            cmd(Code::KeyF),
        ),
        &item(
            MenuAction::StartAwake(Mode::WhileNetworkBusy),
            "While Network Is Busy",
            None,
        ),
        &PredefinedMenuItem::separator(),
        &quick,
        &item(
            MenuAction::OpenAwakeTab,
            "Keep Awake Settings…",
            cmd(Code::Comma),
        ),
    ])?;

    let menu = Menu::new();
    menu.append_items(&[
        &item(MenuAction::OpenDashboard, "Open Dashboard", None),
        &awake_menu,
        &PredefinedMenuItem::separator(),
        &refresh,
        &show_rates,
        &PredefinedMenuItem::separator(),
        &item(MenuAction::Quit, "Quit Netwatch", cmd(Code::KeyQ)),
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
            awake_menu,
            session_line,
            activation_line,
            activation_shown: false,
            allow_display,
            allow_closed,
            screen_saver,
            end_session,
            apps_menu,
            apps: Vec::new(),
            on_launch,
            notify_end,
            login,
            system_checked: None,
            renderer,
            last_icon: None,
            last_tooltip: None,
            last_awake_title: None,
        })
    });
    Ok(())
}

/// The name of an app listed under "While App Is Running".
pub fn app_name(pid: i32) -> Option<String> {
    NATIVE.with(|n| {
        let n = n.borrow();
        let app = n.as_ref()?.apps.iter().find(|a| a.pid == pid)?;
        Some(app.name.clone())
    })
}

/// Redraw the icon, tooltip and menu from the latest sample. Only touches
/// the native tray for parts that changed.
pub fn update(monitor: &Monitor, settings: &Settings, keep_awake: &KeepAwake) {
    NATIVE.with(|n| {
        let mut n = n.borrow_mut();
        let Some(n) = n.as_mut() else { return };
        let now = Instant::now();

        let points = monitor.history().tail(netwatch_core::icon::SLOTS);
        let r = monitor.rates();
        let content = TrayContent {
            points: &points,
            rates: settings.show_rates.then_some((r.down, r.up)),
            disk_used: monitor.disk().map(|d| d.used_fraction()),
            awake: keep_awake.is_active(),
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

        let status = keep_awake.status(now);
        let tip = tooltip(monitor, status.as_deref());
        if n.last_tooltip.as_ref() != Some(&tip) {
            n.tray.set_tooltip(Some(&tip)).ok();
            n.last_tooltip = Some(tip);
        }

        for (ms, item) in &n.intervals {
            item.set_checked(*ms == settings.interval_ms);
        }
        n.show_rates.set_checked(settings.show_rates);
        n.update_awake(settings, keep_awake, status, now);
    });
}

impl Native {
    fn update_awake(
        &mut self,
        settings: &Settings,
        keep_awake: &KeepAwake,
        status: Option<String>,
        now: Instant,
    ) {
        let title = match &status {
            Some(s) => format!("{AWAKE_TITLE}: {s}"),
            None => AWAKE_TITLE.to_string(),
        };
        if self.last_awake_title.as_ref() != Some(&title) {
            self.awake_menu.set_text(&title);
            self.last_awake_title = Some(title);
        }
        self.session_line.set_text(session_line(keep_awake, now));
        // "Manual Activation" shows under the time only while a session runs.
        let activation = keep_awake.session().map(|s| s.activation.label());
        if let Some(text) = activation {
            self.activation_line.set_text(text);
        }
        if activation.is_some() != self.activation_shown {
            let ok = if activation.is_some() {
                self.awake_menu.insert(&self.activation_line, 1)
            } else {
                self.awake_menu.remove(&self.activation_line)
            };
            if ok.is_ok() {
                self.activation_shown = activation.is_some();
            }
        }
        self.end_session.set_enabled(keep_awake.is_active());
        self.allow_display
            .set_checked(settings.awake_allow_display_sleep);
        self.allow_closed
            .set_checked(settings.awake_allow_closed_display_sleep);
        self.screen_saver.set_checked(settings.awake_screen_saver);
        self.screen_saver.set_text(screen_saver_label(settings));
        // The screen saver only needs starting while the display is kept on.
        self.screen_saver
            .set_enabled(!settings.awake_allow_display_sleep);
        self.on_launch.set_checked(settings.awake_on_launch);
        self.notify_end.set_checked(settings.awake_notify_end);

        if self
            .system_checked
            .is_none_or(|t| now.duration_since(t) >= SYSTEM_REFRESH)
        {
            self.system_checked = Some(now);
            self.refresh_apps();
            match system::login::enabled() {
                Some(on) => self.login.set_checked(on),
                None => self.login.set_enabled(false),
            }
        }
    }

    /// Rebuild "While App Is Running" when the set of apps changed.
    fn refresh_apps(&mut self) {
        let apps = system::running_apps();
        if apps == self.apps {
            return;
        }
        while self.apps_menu.remove_at(0).is_some() {}
        if apps.is_empty() {
            let _ = self.apps_menu.append(&label("No Apps Running"));
        }
        for app in &apps {
            let _ = self.apps_menu.append(&item(
                MenuAction::StartAwake(Mode::WhileApp(app.pid)),
                &app.name,
                None,
            ));
        }
        self.apps = apps;
    }
}

/// Force the next [`update`] to re-read the login state and app list, after
/// changing them.
pub fn refresh_system() {
    NATIVE.with(|n| {
        if let Some(n) = n.borrow_mut().as_mut() {
            n.system_checked = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::awake::{Activation, Session};
    use netwatch_core::disk::DiskUsage;
    use netwatch_core::net::InterfaceCounters;

    #[test]
    fn menu_ids_round_trip() {
        for a in [
            MenuAction::OpenDashboard,
            MenuAction::OpenAwakeTab,
            MenuAction::SetInterval(500),
            MenuAction::SetInterval(5000),
            MenuAction::ToggleRates,
            MenuAction::StartAwake(Mode::Indefinite),
            MenuAction::StartAwake(Mode::For(Duration::from_secs(900))),
            MenuAction::StartAwake(Mode::Until(18 * 60 + 30)),
            MenuAction::StartAwake(Mode::WhileApp(4242)),
            MenuAction::StartAwake(Mode::WhileFileDownloading),
            MenuAction::StartAwake(Mode::WhileNetworkBusy),
            MenuAction::EndAwake,
            MenuAction::ToggleAllowDisplaySleep,
            MenuAction::ToggleAllowClosedDisplaySleep,
            MenuAction::ToggleScreenSaver,
            MenuAction::ToggleAwakeOnLaunch,
            MenuAction::ToggleNotifyEnd,
            MenuAction::ToggleLaunchAtLogin,
            MenuAction::Quit,
        ] {
            assert_eq!(MenuAction::parse(&a.id()), Some(a));
        }
        assert_eq!(MenuAction::parse("interval:abc"), None);
        assert_eq!(MenuAction::parse("bogus"), None);
        assert_eq!(MenuAction::parse("awake:start:for:soon"), None);
        assert_eq!(MenuAction::parse("awake:start:nap"), None);
    }

    #[test]
    fn interval_labels() {
        assert_eq!(interval_label(500), "0.5 s");
        assert_eq!(interval_label(2000), "2 s");
    }

    #[test]
    fn session_lines_read_like_amphetamine() {
        let t0 = Instant::now();
        let mut k = KeepAwake::default();
        assert_eq!(session_line(&k, t0), "No Active Session");
        let start = |mode| Session::new(mode, Activation::Manual, None, t0, 0);
        k.start(start(Mode::Indefinite));
        assert_eq!(session_line(&k, t0), "Indeterminate time remaining");
        k.start(start(Mode::For(Duration::from_secs(300))));
        assert_eq!(session_line(&k, t0), "5:00 remaining");
        assert_eq!(
            session_line(&k, t0 + Duration::from_millis(1500)),
            "4:59 remaining"
        );
        k.start(start(Mode::WhileFileDownloading));
        assert_eq!(session_line(&k, t0), "While a file downloads");
        assert_eq!(
            screen_saver_label(&Settings::default()),
            "Allow Screen Saver After 45 min of Inactivity"
        );
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
            tooltip(&m, None),
            "↓ 12.5 MB/s    ↑ 0 B/s\nDisk: 200 GB free of 500 GB (60% used)"
        );
        assert_eq!(
            tooltip(&Monitor::new(10), Some("4:59 left")),
            "↓ 0 B/s    ↑ 0 B/s\nDisk: unavailable\nKeeping awake: 4:59 left"
        );
    }
}
