//! The dashboard popover shown under the status item: live rates, a larger
//! version of the menu-bar graph, link usage and storage.

use crate::stats::{self, Stats};
use crate::theme::{MONO_FONT, UI_FONT};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use netwatch_core::format;
use netwatch_core::history::RatePoint;
use netwatch_core::icon::{GraphFrame, MIN_PEAK, mirrored_graph};
use netwatch_core::monitor::HISTORY_LEN;
use std::time::{Duration, Instant};

pub const WIDTH: f32 = 340.;
pub const HEIGHT: f32 = 486.;
const GRAPH_HEIGHT: f32 = 120.;

pub struct Dashboard {
    stats: Entity<Stats>,
    _subscriptions: [Subscription; 2],
}

impl Dashboard {
    pub fn new(stats: Entity<Stats>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&stats, |_, _, cx| cx.notify());
        // Dismiss like a popover when focus moves elsewhere.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                let s = state(cx);
                s.handle = None;
                s.closed_at = Some(Instant::now());
                window.remove_window();
            }
        });
        Self {
            stats,
            _subscriptions: [observe, activation],
        }
    }
}

/// A capacity bar: the translucent track is the whole, the solid fill the
/// used part.
pub fn meter(fraction: f64, color: Hsla) -> Div {
    div()
        .h(px(8.))
        .w_full()
        .rounded_full()
        .bg(color.opacity(0.22))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0) as f32))
                .rounded_full()
                .bg(color),
        )
}

/// The mirrored rate graph: download above the midline, upload below.
fn graph(points: Vec<RatePoint>, down: Hsla, up: Hsla, grid: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
            let frame = GraphFrame {
                x: 0.0,
                width: w,
                mid: h * 0.62,
                down_h: h * 0.6,
                up_h: h * 0.36,
            };
            let at = |(x, y): (f32, f32)| bounds.origin + point(px(x), px(y));
            window.paint_quad(fill(
                Bounds::new(at((0.0, frame.mid - 0.5)), size(px(w), px(1.0))),
                grid,
            ));
            let g = mirrored_graph(&points, HISTORY_LEN, frame, MIN_PEAK);
            for (poly, color) in [(&g.down, down), (&g.up, up)] {
                if poly.len() < 3 {
                    continue;
                }
                let mut area = PathBuilder::fill();
                area.move_to(at(poly[0]));
                for &p in &poly[1..] {
                    area.line_to(at(p));
                }
                area.close();
                if let Ok(path) = area.build() {
                    window.paint_path(path, color.opacity(0.28));
                }
                // The edge, without the two closing points on the midline.
                let edge = &poly[1..poly.len() - 1];
                if edge.len() >= 2 {
                    let mut line = PathBuilder::stroke(px(1.5));
                    line.move_to(at(edge[0]));
                    for &p in &edge[1..] {
                        line.line_to(at(p));
                    }
                    if let Ok(path) = line.build() {
                        window.paint_path(path, color);
                    }
                }
            }
        },
    )
    .w_full()
    .h(px(GRAPH_HEIGHT))
}

fn section(title: &'static str, detail: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .justify_between()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(detail.into()),
        )
}

fn rate_tile(arrow: &'static str, label: &'static str, rate: f64, color: Hsla, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .gap_0p5()
        .child(
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(div().text_color(color).child(arrow))
                .child(label),
        )
        .child(
            div()
                .font_family(MONO_FONT)
                .text_lg()
                .text_color(cx.theme().foreground)
                .child(format::rate(rate)),
        )
}

fn caption(left: impl Into<SharedString>, right: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .justify_between()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(left.into())
        .child(right.into())
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (down_c, up_c, grid) = (theme.info, theme.success, theme.border);
        let s = self.stats.read(cx);
        let m = &s.monitor;
        let r = m.rates().clone();
        let points: Vec<RatePoint> = m.history().iter().copied().collect();
        let peak = netwatch_core::history::History::peak(&points);
        let span = Duration::from_millis(s.settings.interval_ms) * m.history().capacity() as u32;
        let (session_down, session_up) = m.session_totals();
        let disk = m.disk();

        let link_detail = match &r.link {
            Some(l) if l.capacity_bps > 0 => {
                format!("{} · {}", l.name, format::bits_per_sec(l.capacity_bps))
            }
            Some(l) => l.name.clone(),
            None => "offline".into(),
        };

        let mut body = v_flex()
            .gap_3()
            .child(section("Network", link_detail, cx))
            .child(
                h_flex()
                    .gap_3()
                    .child(rate_tile("↓", "Download", r.down, down_c, cx))
                    .child(rate_tile("↑", "Upload", r.up, up_c, cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(graph(points, down_c, up_c, grid))
                    .child(caption(
                        format!("peak {}", format::rate(peak)),
                        format!("last {} s", span.as_secs()),
                        cx,
                    )),
            );

        // Link usage: how much of the connection's capacity is in use.
        if let (Some(link), Some(u)) = (&r.link, r.utilization()) {
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(caption(
                        "Link usage",
                        format!(
                            "{} of {}",
                            format::percent(u),
                            format::bits_per_sec(link.capacity_bps)
                        ),
                        cx,
                    ))
                    .child(meter(u, down_c)),
            );
        }
        body = body.child(caption(
            "This session",
            format!(
                "↓ {}   ↑ {}",
                format::bytes(session_down),
                format::bytes(session_up)
            ),
            cx,
        ));

        let storage = match disk {
            Some(d) => {
                let f = d.used_fraction();
                let color = if f >= 0.9 {
                    theme.danger
                } else if f >= 0.8 {
                    theme.warning
                } else {
                    theme.primary
                };
                v_flex()
                    .gap_2()
                    .child(section("Storage", format::bytes(d.total), cx))
                    .child(meter(f, color))
                    .child(caption(
                        format!("{} used ({})", format::bytes(d.used()), format::percent(f)),
                        format!("{} free", format::bytes(d.available)),
                        cx,
                    ))
            }
            None => v_flex()
                .gap_2()
                .child(section("Storage", "unavailable", cx)),
        };

        v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .font_family(UI_FONT)
            .text_sm()
            .text_color(theme.foreground)
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(px(12.))
            .child(body)
            .child(div().h(px(1.)).bg(theme.border))
            .child(storage)
            .child(
                h_flex()
                    .mt_auto()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Updates every {}",
                        crate::tray::interval_label(s.settings.interval_ms)
                    ))
                    .child(
                        Button::new("quit")
                            .ghost()
                            .xsmall()
                            .label("Quit")
                            .on_click(|_, _, cx| cx.quit()),
                    ),
            )
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
}

impl Global for DashboardWindow {}

const REOPEN_GUARD: Duration = Duration::from_millis(300);

fn state(cx: &mut App) -> &mut DashboardWindow {
    if !cx.has_global::<DashboardWindow>() {
        cx.set_global(DashboardWindow::default());
    }
    cx.global_mut::<DashboardWindow>()
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
    let placement = crate::placement::below_menu_bar(WIDTH as f64);
    let display_id = placement
        .and_then(|p| p.display_id)
        .map(|id| DisplayId::new(id as u64))
        .filter(|id| cx.find_display(*id).is_some());
    let origin = placement
        .map(|p| point(px(p.x as f32), px(p.y as f32)))
        .unwrap_or_else(|| point(px(100.), px(40.)));
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
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| Dashboard::new(stats, window, cx))
    }) {
        Ok(handle) => {
            state(cx).handle = Some(handle);
            cx.activate(true);
        }
        Err(e) => log::error!("Failed to open the dashboard: {e}"),
    }
}
