//! The dashboard popover shown under the status item, in four tabs:
//! Overview (rates and storage at a glance), Network (link, session totals
//! and the speed test), Storage (used space over time) and Settings.
//! The look follows `design/dashboard-mock.html`; the pieces live in
//! [`crate::ui`].

use crate::stats::{self, SpeedTest, Stats};
use crate::ui::{self, color::*, px};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use netwatch_core::disk::DiskUsage;
use netwatch_core::disk_history::{self, DiskSample};
use netwatch_core::format;
use netwatch_core::history::{self, History, RatePoint};
use netwatch_core::icon::MIN_PEAK;
use netwatch_core::net::NetRates;
use netwatch_core::settings::{INTERVALS_MS, Settings};
use netwatch_core::speedtest;
use std::time::{Duration, Instant};

pub const WIDTH: f32 = ui::CARD_W + 2. * ui::SHADOW;
/// Tall enough for the longest tab (Network); the rest of the window is
/// transparent, and clicking it closes the dashboard.
pub const HEIGHT: f32 = 800.;
/// Bars in the rate graph; each covers `HISTORY_LEN / BARS` samples.
const BARS: usize = 40;
const CHART_H: f32 = 210.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Overview,
    Network,
    Storage,
    Settings,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Overview, Tab::Network, Tab::Storage, Tab::Settings];

    fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Network => "Network",
            Tab::Storage => "Storage",
            Tab::Settings => "Settings",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn next(self) -> Tab {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn parse(name: &str) -> Option<Tab> {
        Self::ALL
            .into_iter()
            .find(|t| t.label().eq_ignore_ascii_case(name))
    }
}

pub struct Dashboard {
    stats: Entity<Stats>,
    tab: Tab,
    interval_menu_open: bool,
    /// Whether the speed test section on Network is unfolded.
    speed_test_open: bool,
    _subscriptions: [Subscription; 2],
}

impl Dashboard {
    pub fn new(stats: Entity<Stats>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&stats, |_, _, cx| cx.notify());
        // Dismiss like a popover when focus moves elsewhere. Debug builds
        // can pin it open with NETWATCH_DEV_PIN, for inspecting it.
        let pinned = cfg!(debug_assertions) && std::env::var_os("NETWATCH_DEV_PIN").is_some();
        let activation = cx.observe_window_activation(window, move |_, window, cx| {
            if !window.is_window_active() && !pinned {
                dismiss(window, cx);
            }
        });
        Self {
            stats,
            tab: state(cx).tab,
            speed_test_open: state(cx).speed_test_open,
            interval_menu_open: false,
            _subscriptions: [observe, activation],
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.interval_menu_open = false;
        state(cx).tab = tab;
        cx.notify();
    }
}

/// What the tabs show, copied out of [`Stats`] so rendering can borrow the
/// view mutably for its listeners.
struct Snapshot {
    rates: NetRates,
    points: Vec<RatePoint>,
    capacity: usize,
    settings: Settings,
    session: (u64, u64),
    disk: Option<DiskUsage>,
    disk_samples: Vec<DiskSample>,
    speed_test: SpeedTest,
}

impl Snapshot {
    fn read(stats: &Stats) -> Self {
        let m = &stats.monitor;
        let disk = m.disk();
        Self {
            rates: m.rates().clone(),
            points: m.history().iter().copied().collect(),
            capacity: m.history().capacity(),
            settings: stats.settings.clone(),
            session: m.session_totals(),
            disk,
            disk_samples: stats.disk_history.with_live(disk_history::unix_now(), disk),
            speed_test: stats.speed_test.clone(),
        }
    }

    /// How far back the rate graph reaches.
    fn graph_span(&self) -> Duration {
        Duration::from_millis(self.settings.interval_ms) * self.capacity as u32
    }

    /// First and last stored disk samples, when there are two to compare.
    fn disk_range(&self) -> Option<(DiskSample, DiskSample)> {
        match (self.disk_samples.first(), self.disk_samples.last()) {
            (Some(a), Some(b)) if self.disk_samples.len() >= 2 && b.at > a.at => Some((*a, *b)),
            _ => None,
        }
    }
}

fn live_label(rates: &NetRates) -> AnyElement {
    match rates.link {
        Some(_) => div().text_color(ok()).child("live").into_any_element(),
        None => div()
            .text_color(danger())
            .child("offline")
            .into_any_element(),
    }
}

fn text(s: impl Into<SharedString>) -> AnyElement {
    div().child(s.into()).into_any_element()
}

/// The bar graph with its caption, and the live rates under it. Shared by
/// Overview and Network.
fn rates_section(s: &Snapshot) -> Div {
    let peak = History::peak(&s.points);
    v_flex()
        .gap(px(22.))
        .child(
            v_flex()
                .gap(px(10.))
                .child(ui::rate_bars(
                    history::bars(&s.points, s.capacity, BARS),
                    peak.max(MIN_PEAK),
                ))
                .child(ui::caption(
                    ui::mono(13., fg3()).child(format!("peak {}", format::rate(peak))),
                    ui::mono(13., fg3())
                        .child(format!("last {}", format::span(s.graph_span().as_secs()))),
                )),
        )
        .child(
            h_flex()
                .gap(px(14.))
                .child(ui::rate_box("arrow-down", &format::rate(s.rates.down)))
                .child(ui::rate_box("arrow-up", &format::rate(s.rates.up))),
        )
}

/// Used of total as a bar, with free space and the change over the stored
/// history. Shared by Overview and Storage.
fn storage_section(s: &Snapshot) -> Div {
    let Some(d) = s.disk else {
        return v_flex().child(ui::label_row(
            "Storage",
            ui::ui_text(14.5, fg3()).child("unavailable"),
        ));
    };
    let change = match s.disk_range() {
        Some((first, last)) => format!(
            "{} in {}",
            format::bytes_delta(last.used as i64 - first.used as i64),
            format::span(last.at - first.at)
        ),
        None => "collecting…".into(),
    };
    v_flex()
        .child(ui::label_row(
            "Storage",
            h_flex()
                .items_baseline()
                .gap(px(6.))
                .child(ui::mono(15., fg()).child(format::bytes(d.used())))
                .child(ui::mono(15., fg2()).child(format!("of {}", format::bytes(d.total)))),
        ))
        .child(div().h(px(12.)))
        .child(ui::storage_bar(
            d.used_fraction(),
            ui::level_color(s.settings.storage_level(d.used_fraction())),
        ))
        .child(div().h(px(10.)))
        .child(ui::caption(
            ui::dotted(vec![
                text(format!("{} used", format::percent(d.used_fraction()))),
                text(format!("{} free", format::bytes(d.available))),
            ]),
            div().text_color(fg2()).child(change),
        ))
}

impl Dashboard {
    fn header(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        let link = s.rates.link.as_ref();
        let (icon, title, subtitle, chip): (&str, SharedString, Vec<AnyElement>, _) = match self.tab
        {
            Tab::Overview => {
                let mut parts = vec![];
                if let Some(l) = link {
                    parts.push(text(l.name.clone()));
                }
                parts.push(live_label(&s.rates));
                if let Some(d) = s.disk {
                    parts.push(text(format!("{} free", format::bytes(d.available))));
                }
                ("layout-grid", "Netwatch".into(), parts, None)
            }
            Tab::Network => {
                let mut parts = vec![];
                if let Some(l) = link {
                    parts.push(text(l.name.clone()));
                    if l.capacity_bps > 0 {
                        parts.push(text(format::bits_per_sec(l.capacity_bps)));
                    }
                }
                parts.push(live_label(&s.rates));
                ("activity", "Network".into(), parts, None)
            }
            Tab::Storage => {
                let mut parts = vec![text("Startup disk")];
                if let Some(d) = s.disk {
                    parts.push(text(format::bytes(d.total)));
                }
                let chip = s
                    .disk_range()
                    .map(|(a, b)| SharedString::from(format::span(b.at - a.at)));
                ("hard-drive", "Storage".into(), parts, chip)
            }
            Tab::Settings => (
                "sliders-horizontal",
                "Settings".into(),
                vec![
                    text(netwatch_core::PRODUCT_NAME),
                    text(env!("CARGO_PKG_VERSION")),
                ],
                None,
            ),
        };
        let dot = if link.is_some() { ok() } else { danger() };
        ui::header(
            ui::icon_button("tab-icon", ui::header_icon(icon, dot)),
            title,
            ui::dotted(subtitle),
            chip,
            ui::icon_button("next-tab", ui::icon("chevron-right", 18., fg()))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0x222222)))
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(this.tab.next(), cx))),
        )
    }

    fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let segments = Tab::ALL
            .iter()
            .map(|&t| {
                div()
                    .id(t.label())
                    .child(t.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
            })
            .collect();
        ui::segmented(segments, self.tab.index())
    }

    fn overview(&self, s: &Snapshot) -> Div {
        v_flex()
            .gap(px(22.))
            .child(rates_section(s))
            .child(storage_section(s))
    }

    fn network(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        let link: AnyElement = match &s.rates.link {
            Some(l) => {
                let mut parts = vec![text(l.name.clone())];
                if l.capacity_bps > 0 {
                    parts.push(
                        div()
                            .text_color(fg3())
                            .child(format::bits_per_sec(l.capacity_bps))
                            .into_any_element(),
                    );
                }
                if let Some(u) = s.rates.utilization() {
                    parts.push(text(format!("{} used", format::percent(u))));
                }
                ui::dotted(parts).into_any_element()
            }
            None => div()
                .text_color(danger())
                .child("offline")
                .into_any_element(),
        };
        let (down, up) = s.session;
        v_flex()
            .gap(px(22.))
            .child(rates_section(s))
            .child(
                v_flex()
                    .gap(px(10.))
                    .child(ui::kv("Link", link))
                    .child(ui::kv(
                        "This session",
                        ui::dotted(vec![
                            text(format!("↓ {}", format::bytes(down))),
                            text(format!("↑ {}", format::bytes(up))),
                        ]),
                    )),
            )
            .child(ui::separator())
            .child(self.speed_test(s, cx))
    }

    /// Folds to one summary line; unfolded it shows the bars, latency and
    /// the Run test button.
    fn speed_test(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        let open = self.speed_test_open;
        let head = h_flex()
            .id("speed-test-head")
            .justify_between()
            .cursor_pointer()
            .child(ui::ui_text(14.5, fg2()).child("Speed test"))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(speed_test_status(s, open))
                    .child(ui::icon(
                        if open { "chevron-up" } else { "chevron-down" },
                        14.,
                        fg3(),
                    )),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.speed_test_open = !this.speed_test_open;
                state(cx).speed_test_open = this.speed_test_open;
                cx.notify();
            }));
        v_flex().gap(px(12.)).child(head).when(open, |v| {
            v.child(speed_test_body(s))
                .child(div().h(px(10.)))
                .child(speed_test_button(&s.speed_test, cx))
        })
    }

    fn storage(&self, s: &Snapshot) -> Div {
        v_flex()
            .gap(px(22.))
            .child(storage_chart(s))
            .child(storage_section(s))
    }

    fn settings(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        let show_rates = s.settings.show_rates;
        v_flex()
            .gap(px(22.))
            .child(
                v_flex()
                    .child(setting_row(
                        "Refresh every",
                        "How often the icon updates",
                        true,
                        self.interval_dropdown(&s.settings, cx),
                    ))
                    .child(setting_row(
                        "Show speeds in menu bar",
                        "Rates as text next to the icon",
                        false,
                        ui::toggle("show-rates", show_rates).on_click(|_, _, cx| {
                            crate::update_settings(cx, |s| s.show_rates = !s.show_rates);
                        }),
                    )),
            )
            .child(storage_colors(s))
            .child(
                h_flex()
                    .gap(px(14.))
                    .child(ui::button("reset", "Reset", false).on_click(|_, _, cx| {
                        crate::update_settings(cx, |s| *s = Settings::default());
                    }))
                    .child(ui::button("quit", "Quit", true).on_click(|_, _, cx| cx.quit())),
            )
    }

    fn interval_dropdown(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let open = self.interval_menu_open;
        let pill = h_flex()
            .id("interval")
            .h(px(33.))
            .min_w(px(104.))
            .px(px(12.))
            .gap(px(8.))
            .justify_between()
            .rounded(px(8.))
            .border_1()
            .border_color(if open { focus() } else { card_border() })
            .text_size(px(15.))
            .cursor_pointer()
            .child(crate::tray::interval_label(settings.interval_ms))
            .child(ui::icon(
                if open { "chevron-up" } else { "chevron-down" },
                12.,
                fg2(),
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.interval_menu_open = !this.interval_menu_open;
                cx.notify();
            }));

        let menu = open.then(|| {
            let items = INTERVALS_MS.iter().map(|&ms| {
                let selected = ms == settings.interval_ms;
                // The graph keeps a fixed number of samples, so the interval
                // sets how far back it reaches.
                let reach = Duration::from_millis(ms) * netwatch_core::monitor::HISTORY_LEN as u32;
                h_flex()
                    .id(ElementId::Integer(ms))
                    .h(px(33.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(7.))
                    .text_size(px(15.))
                    .text_color(if selected { fg() } else { fg2() })
                    .cursor_pointer()
                    .hover(|s| s.bg(raised()).text_color(fg()))
                    .child(div().flex_1().child(crate::tray::interval_label(ms)))
                    .child(
                        ui::mono(13.5, fg3())
                            .child(format!("{} graph", format::span(reach.as_secs()))),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .children(selected.then(|| ui::icon("check", 14., ok()))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.interval_menu_open = false;
                        crate::update_settings(cx, |s| s.interval_ms = ms);
                    }))
            });
            deferred(
                v_flex()
                    .absolute()
                    .top(px(40.))
                    .right_0()
                    .w(px(220.))
                    .p(px(6.))
                    .bg(rgb(0x141414))
                    .border_1()
                    .border_color(card_border())
                    .rounded(px(12.))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., 0.6),
                        offset: point(px(0.), px(16.)),
                        blur_radius: px(40.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .occlude()
                    .children(items),
            )
            .with_priority(1)
        });

        div()
            .relative()
            .flex_none()
            .child(pill)
            .children(menu)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.interval_menu_open {
                    this.interval_menu_open = false;
                    cx.notify();
                }
            }))
    }
}

/// Where the storage bar turns yellow and red, and the red-zone alert.
fn storage_colors(s: &Snapshot) -> Div {
    const STEP: u8 = 5;
    let (yellow, red) = s.settings.storage_thresholds();
    let (y, r) = ((yellow * 100.0).round() as u8, (red * 100.0).round() as u8);
    let set = |f: fn(&mut Settings, u8), v: u8| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            crate::update_settings(cx, |s| f(s, v));
        }
    };
    let set_yellow: fn(&mut Settings, u8) = |s, v| s.storage_warn_pct = v;
    let set_red: fn(&mut Settings, u8) = |s, v| s.storage_danger_pct = v;
    let threshold = |label: &str, stepper: Div| {
        v_flex()
            .flex_1()
            .gap(px(8.))
            .child(ui::ui_text(14.5, fg2()).child(label.to_string()))
            .child(stepper)
    };
    let notify = s.settings.storage_notify;
    v_flex()
        .gap(px(12.))
        .child(ui::label_row(
            "Storage colours",
            ui::ui_text(14.5, fg3()).child("% used"),
        ))
        .child(ui::zone_bar(
            yellow,
            red,
            s.disk.map_or(0.0, |d| d.used_fraction()),
        ))
        .child(
            h_flex()
                .gap(px(14.))
                .child(threshold(
                    "Yellow from",
                    ui::stepper(
                        "yellow",
                        format!("{y}%"),
                        (y > STEP).then(|| set(set_yellow, y - STEP)),
                        (y + STEP < r).then(|| set(set_yellow, y + STEP)),
                    ),
                ))
                .child(threshold(
                    "Red from",
                    ui::stepper(
                        "red",
                        format!("{r}%"),
                        (r > y + STEP).then(|| set(set_red, r - STEP)),
                        (r + STEP <= 95).then(|| set(set_red, r + STEP)),
                    ),
                )),
        )
        .child(setting_row(
            "Notify in the red",
            &format!("When the startup disk passes {r}%"),
            false,
            ui::toggle("storage-notify", notify).on_click(|_, _, cx| {
                crate::update_settings(cx, |s| s.storage_notify = !s.storage_notify);
            }),
        ))
}

fn setting_row(title: &str, detail: &str, first: bool, control: impl IntoElement) -> Div {
    h_flex()
        .gap(px(16.))
        .py(px(14.))
        .when(first, |r| r.pt(px(2.)))
        .when(!first, |r| r.border_t_1().border_color(sep()))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(ui::ui_text(17., fg()).child(title.to_string()))
                .child(ui::ui_text(14.5, fg3()).child(detail.to_string())),
        )
        .child(control)
}

/// Mbps as a short number for the bar values: `412`, `38.6`, `4.21`.
fn mbps(bps: u64) -> String {
    let v = bps as f64 / 1e6;
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// Right side of the speed test header. Folded, it carries the result.
fn speed_test_status(s: &Snapshot, open: bool) -> Div {
    let status: AnyElement = match &s.speed_test {
        SpeedTest::Idle => text("not run yet"),
        SpeedTest::Running { started } => div()
            .text_color(caution())
            .child(format!("testing… {} s", started.elapsed().as_secs()))
            .into_any_element(),
        SpeedTest::Done { finished, .. } => text(format!(
            "{} ago",
            format::span(finished.elapsed().as_secs())
        )),
        SpeedTest::Failed(_) => div()
            .text_color(danger())
            .child("failed")
            .into_any_element(),
    };
    let parts = match &s.speed_test {
        SpeedTest::Done { result, .. } if !open => vec![
            text(format!("↓ {}", mbps(result.down_bps))),
            text(format!("↑ {} Mbps", mbps(result.up_bps))),
            status,
        ],
        // Units only when there are numbers to read them against.
        SpeedTest::Done { .. } => vec![text("Mbps"), status],
        _ => vec![status],
    };
    ui::dotted(parts).text_size(px(14.5)).text_color(fg3())
}

/// The last result next to the link rate, and latency idle vs under load.
fn speed_test_body(s: &Snapshot) -> Div {
    match &s.speed_test {
        SpeedTest::Done { result, .. } => {
            let cap = s.rates.link.as_ref().map_or(0, |l| l.capacity_bps);
            let scale = cap.max(result.down_bps).max(result.up_bps).max(1) as f64;
            let mut bars = v_flex().gap(px(11.));
            if cap > 0 {
                bars = bars.child(ui::hbar("Link rate", cap as f64 / scale, bar(), mbps(cap)));
            }
            bars = bars
                .child(ui::hbar(
                    "Download",
                    result.down_bps as f64 / scale,
                    bar_lite(),
                    mbps(result.down_bps),
                ))
                .child(ui::hbar(
                    "Upload",
                    result.up_bps as f64 / scale,
                    bar_lite(),
                    mbps(result.up_bps),
                ));
            let mut body = v_flex().gap(px(14.)).child(bars);
            if let Some(latency) = latency(result) {
                body = body.child(ui::kv("Latency", latency));
            }
            body
        }
        SpeedTest::Failed(e) => v_flex().child(ui::ui_text(14.5, fg3()).child(e.clone())),
        SpeedTest::Idle | SpeedTest::Running { .. } => v_flex()
            .child(ui::ui_text(14.5, fg3()).child("Measures against Apple's servers, about 20 s")),
    }
}

/// `18 ms idle · 48 ms loaded · Good`. `networkQuality` reports
/// responsiveness as round trips per minute under load, so 60000 / RPM is the
/// loaded round-trip time in ms.
fn latency(r: &speedtest::SpeedTestResult) -> Option<Div> {
    let mut parts = vec![];
    if let Some(ms) = r.idle_latency_ms {
        parts.push(text(format!("{ms:.0} ms idle")));
    }
    if let Some(rpm) = r.responsiveness_rpm.filter(|r| *r > 0.0) {
        let loaded = 60_000.0 / rpm;
        parts.push(text(format!("{loaded:.0} ms loaded")));
        let (label, color) = match loaded {
            l if l < 60.0 => ("Good", ok()),
            l if l < 150.0 => ("Fair", caution()),
            _ => ("Poor", danger()),
        };
        parts.push(div().text_color(color).child(label).into_any_element());
    }
    (!parts.is_empty()).then(|| ui::dotted(parts))
}

fn speed_test_button(state: &SpeedTest, cx: &mut Context<Dashboard>) -> Div {
    let start = |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        stats::stats(cx).update(cx, |s, cx| s.start_speed_test(cx));
    };
    let _ = cx;
    let button = match state {
        SpeedTest::Running { started } => {
            let elapsed = started.elapsed();
            let progress = elapsed.as_secs_f32() / speedtest::MAX_RUNTIME.as_secs_f32();
            ui::button(
                "speed-test",
                format!("Testing · {} s", elapsed.as_secs()),
                false,
            )
            .text_color(fg())
            .cursor_default()
            // The button itself fills as the test runs. A full-height fill
            // can share the button's rounded corners; a thin line along the
            // bottom edge can't, since clipping ignores the rounding.
            .child({
                let p = progress.clamp(0.0, 1.0);
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(relative(p))
                    .rounded_l(px(11.))
                    .when(p > 0.98, |d| d.rounded_r(px(11.)))
                    .bg(ok().opacity(0.16))
            })
        }
        _ => ui::button("speed-test", "Run test", true).on_click(start),
    };
    h_flex().child(button)
}

/// Used space over the stored history: the line, a dashed straight trend
/// from first to last, and the two end values.
fn storage_chart(s: &Snapshot) -> Div {
    const PAD_X: f32 = 24.;
    const PAD_TOP: f32 = 30.;
    const PAD_BOTTOM: f32 = 40.;
    let w = ui::CONTENT_W - 2.;
    let h = CHART_H - 2.;
    let frame = div()
        .relative()
        .w_full()
        .h(px(CHART_H))
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(0x1c1c1c))
        .bg(rgb(0x0c0c0c))
        .overflow_hidden();

    let Some((first, last)) = s.disk_range() else {
        return frame.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::ui_text(14.5, fg3()).child("Collecting history…")),
        );
    };

    let pts: Vec<(f32, f32)> =
        disk_history::graph_points(&s.disk_samples, w - 2. * PAD_X, h - PAD_TOP - PAD_BOTTOM)
            .into_iter()
            .map(|(x, y)| (x + PAD_X, y + PAD_TOP))
            .collect();
    let (x0, y0) = pts[0];
    let (x1, y1) = pts[pts.len() - 1];
    let span = last.at - first.at;
    let delta = last.used as f64 - first.used as f64;
    let (per, unit) = if span >= 86_400 {
        (86_400.0, "day")
    } else {
        (3_600.0, "h")
    };
    let rate = format!(
        "avg {}/{unit}",
        format::bytes_delta((delta * per / span as f64) as i64)
    );

    let plot = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |(x, y): (f32, f32)| bounds.origin + point(px(x), px(y));
            let (bw, bh) = (
                bounds.size.width.as_f32() / ui::SCALE,
                bounds.size.height.as_f32() / ui::SCALE,
            );
            // Grid: fine every 13.5 px, coarse every 54.
            let mut x = 0.0;
            let mut i = 0;
            while x < bw {
                let c = if i % 4 == 0 {
                    rgb(0x1a1a1a)
                } else {
                    rgb(0x141414)
                };
                window.paint_quad(fill(Bounds::new(at((x, 0.)), size(px(1.), px(bh))), c));
                x += 13.5;
                i += 1;
            }
            let (mut y, mut i) = (0.0, 0);
            while y < bh {
                let c = if i % 4 == 0 {
                    rgb(0x1a1a1a)
                } else {
                    rgb(0x141414)
                };
                window.paint_quad(fill(Bounds::new(at((0., y)), size(px(bw), px(1.))), c));
                y += 13.5;
                i += 1;
            }
            let line = |from: (f32, f32), to: (f32, f32), dash: &[Pixels], width: f32| {
                let mut b = PathBuilder::stroke(px(width)).dash_array(dash);
                b.move_to(at(from));
                b.line_to(at(to));
                b.build().ok()
            };
            // Guides at the first and last values.
            if let Some(p) = line((x0, y0), (bw - 10., y0), &[px(2.), px(4.)], 1.) {
                window.paint_path(p, rgb(0x555555));
            }
            if let Some(p) = line((PAD_X + 20., y1), (x1, y1), &[px(2.), px(4.)], 1.) {
                window.paint_path(p, rgb(0x555555));
            }
            if let Some(p) = line((x0, y0), (x1, y1), &[px(4.), px(4.)], 1.3) {
                window.paint_path(p, caution().opacity(0.85));
            }
            let mut b = PathBuilder::stroke(px(2.));
            b.move_to(at(pts[0]));
            for &p in &pts[1..] {
                b.line_to(at(p));
            }
            if let Ok(p) = b.build() {
                window.paint_path(p, ok());
            }
            for (cx, cy) in [(x0, y0), (x1, y1)] {
                for (r, c) in [(5.5, ok()), (3.5, rgb(0x0c0c0c).into())] {
                    window.paint_quad(
                        fill(
                            Bounds::new(at((cx - r, cy - r)), size(px(2. * r), px(2. * r))),
                            c,
                        )
                        .corner_radii(px(r)),
                    );
                }
            }
        },
    )
    .absolute()
    .inset_0();

    let label = |s: String| ui::mono(12., rgb(0x8a8a8a).into()).absolute().child(s);
    frame
        .child(plot)
        .child(
            label(format::bytes(last.used))
                .left(px(PAD_X + 20.))
                .top(px((y1 - 20.).max(4.))),
        )
        .child(
            label(format::bytes(first.used))
                .right(px(10.))
                .top(px((y0 - 20.).max(4.))),
        )
        .child(
            h_flex()
                .absolute()
                .left(px(14.))
                .right(px(14.))
                .bottom(px(12.))
                .justify_between()
                .font_family(crate::theme::MONO_FONT)
                .text_size(px(13.))
                .text_color(rgb(0x6a6a6a))
                .child(format!("{} ago", format::span(span)))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_color(rgb(0x8a8a8a))
                        .child(div().text_color(caution()).child("- -"))
                        .child(rate),
                )
                .child("now"),
        )
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = Snapshot::read(self.stats.read(cx));
        let body = match self.tab {
            Tab::Overview => self.overview(&s),
            Tab::Network => self.network(&s, cx),
            Tab::Storage => self.storage(&s),
            Tab::Settings => self.settings(&s, cx),
        };
        let card = ui::card(
            self.header(&s, cx),
            v_flex().gap(px(22.)).child(self.tabs(cx)).child(body),
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        // The window is taller than most tabs; a click on the transparent
        // part below the card closes the dashboard like a click elsewhere.
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .px(px(ui::SHADOW))
            .on_mouse_down(MouseButton::Left, |_, window, cx| dismiss(window, cx))
            .child(card)
    }
}

/// Open/close bookkeeping for the single dashboard window.
#[derive(Default)]
struct DashboardWindow {
    handle: Option<WindowHandle<Dashboard>>,
    /// When it last closed by losing focus. Clicking the status item while
    /// the dashboard is open first deactivates it (closing it) and then
    /// delivers the click, which must not reopen it.
    closed_at: Option<Instant>,
    /// Reopen on the tab that was showing, with the speed test folded the
    /// same way.
    tab: Tab,
    speed_test_open: bool,
}

impl Global for DashboardWindow {}

const REOPEN_GUARD: Duration = Duration::from_millis(300);

fn state(cx: &mut App) -> &mut DashboardWindow {
    if !cx.has_global::<DashboardWindow>() {
        cx.set_global(DashboardWindow::default());
    }
    cx.global_mut::<DashboardWindow>()
}

fn dismiss(window: &mut Window, cx: &mut App) {
    let s = state(cx);
    s.handle = None;
    s.closed_at = Some(Instant::now());
    window.remove_window();
}

/// The tab the dashboard opens on next.
pub fn set_tab(tab: Tab, cx: &mut App) {
    state(cx).tab = tab;
}

pub fn is_open(cx: &mut App) -> bool {
    state(cx).handle.is_some()
}

/// Status-item click: open, or close if already open.
pub fn toggle(cx: &mut App) {
    if is_open(cx) {
        close(cx);
        return;
    }
    let recently_closed = state(cx)
        .closed_at
        .is_some_and(|t| t.elapsed() < REOPEN_GUARD);
    if !recently_closed {
        open(cx);
    }
}

pub fn close(cx: &mut App) {
    if let Some(handle) = state(cx).handle.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

pub fn open(cx: &mut App) {
    if let Some(handle) = state(cx).handle {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return;
    }
    crate::theme::follow_system(cx);
    let placement = crate::placement::below_menu_bar((WIDTH * ui::SCALE) as f64);
    let display_id = placement
        .and_then(|p| p.display_id)
        .map(|id| DisplayId::new(id as u64))
        .filter(|id| cx.find_display(*id).is_some());
    let origin = placement
        // Screen coordinates, so not scaled.
        .map(|p| point(gpui_kit::px(p.x as f32), gpui_kit::px(p.y as f32)))
        .unwrap_or_else(|| point(gpui_kit::px(100.), gpui_kit::px(40.)));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin,
            size: size(px(WIDTH), px(HEIGHT)),
        })),
        display_id,
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(netwatch_core::APP_IDENTIFIER.into()),
        ..Default::default()
    };
    let stats = stats::stats(cx);
    let mut native = None;
    match cx.open_window(options, |window, cx| {
        native = native_window(window);
        cx.new(|cx| Dashboard::new(stats, window, cx))
    }) {
        Ok(handle) => {
            state(cx).handle = Some(handle);
            cx.activate(true);
            // Restyling makes AppKit call back into GPUI, so do it once GPUI
            // is done updating, from the run loop.
            if let Some(native) = native {
                cx.foreground_executor()
                    .spawn(async move { strip_window_chrome(&native) })
                    .detach();
            }
        }
        Err(e) => log::error!("Failed to open the dashboard: {e}"),
    }
}

#[cfg(target_os = "macos")]
type NativeWindow = objc2::rc::Retained<objc2_app_kit::NSWindow>;
#[cfg(not(target_os = "macos"))]
type NativeWindow = ();

#[cfg(target_os = "macos")]
fn native_window(window: &Window) -> Option<NativeWindow> {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI hands out its live content view, and we're on the main
    // thread inside its window callback.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}

#[cfg(not(target_os = "macos"))]
fn native_window(_: &Window) -> Option<NativeWindow> {
    None
}

/// Make the popover window draw nothing but the card. GPUI's transparent
/// windows keep a near-invisible background and a titled frame, so macOS
/// still draws a rim and a shadow around the whole window rectangle, which
/// is taller than most tabs. Going borderless without the system shadow
/// leaves only what we paint (the card brings its own shadow).
#[cfg(target_os = "macos")]
fn strip_window_chrome(window: &NativeWindow) {
    use objc2_app_kit::NSWindowStyleMask;
    let keep = window.styleMask() & NSWindowStyleMask::NonactivatingPanel;
    window.setStyleMask(NSWindowStyleMask::Borderless | keep);
    window.setHasShadow(false);
}

#[cfg(not(target_os = "macos"))]
fn strip_window_chrome(_: &NativeWindow) {}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob export includes its own `test` attribute.
    use super::{Tab, mbps};

    #[test]
    fn tabs_cycle() {
        assert_eq!(Tab::Overview.next(), Tab::Network);
        assert_eq!(Tab::Settings.next(), Tab::Overview);
        assert_eq!(Tab::Storage.index(), 2);
        assert_eq!(Tab::parse("settings"), Some(Tab::Settings));
        assert_eq!(Tab::parse("nope"), None);
    }

    #[test]
    fn mbps_keeps_three_digits() {
        assert_eq!(mbps(412_300_000), "412");
        assert_eq!(mbps(38_600_000), "38.6");
        assert_eq!(mbps(4_210_000), "4.21");
    }
}
