//! The dashboard's visual language: a fixed dark palette and the building
//! blocks the tabs are made of. Mirrors `design/dashboard-mock.html`.

use crate::theme::{MONO_FONT, UI_FONT};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use utilify_core::history::RatePoint;
use utilify_core::settings::StorageLevel;

pub mod color {
    use gpui_kit::{Hsla, rgb};

    fn c(hex: u32) -> Hsla {
        rgb(hex).into()
    }
    pub fn card() -> Hsla {
        c(0x191919)
    }
    pub fn card_border() -> Hsla {
        c(0x2a2a2a)
    }
    pub fn panel() -> Hsla {
        c(0x0f0f0f)
    }
    pub fn panel_border() -> Hsla {
        c(0x1e1e1e)
    }
    pub fn control() -> Hsla {
        c(0x131313)
    }
    pub fn control_border() -> Hsla {
        c(0x282828)
    }
    pub fn raised() -> Hsla {
        c(0x1f1f1f)
    }
    pub fn raised_border() -> Hsla {
        c(0x303030)
    }
    pub fn sep() -> Hsla {
        c(0x222222)
    }
    pub fn fg() -> Hsla {
        c(0xf2f2f2)
    }
    pub fn fg2() -> Hsla {
        c(0xa3a3a3)
    }
    pub fn fg3() -> Hsla {
        c(0x7a7a7a)
    }
    pub fn fg4() -> Hsla {
        c(0x555555)
    }
    pub fn bar() -> Hsla {
        c(0x5c5c5c)
    }
    pub fn bar_lite() -> Hsla {
        c(0xd8d8d8)
    }
    pub fn track() -> Hsla {
        c(0x1d1d1d)
    }
    pub fn up_bar() -> Hsla {
        c(0x3a7a52)
    }
    pub fn ok() -> Hsla {
        c(0x4fcf7d)
    }
    pub fn caution() -> Hsla {
        c(0xe8b54a)
    }
    pub fn danger() -> Hsla {
        c(0xf0675c)
    }
    pub fn focus() -> Hsla {
        c(0x2f8a53)
    }
}

use color::*;

/// Overall size of the dashboard. Every length in it is written at design
/// scale (the mock's numbers) and goes through [`px`], which applies this.
pub const SCALE: f32 = 0.7;

/// Design-scale pixels: shadows `gpui_kit::px` here and in the dashboard so
/// the whole UI follows [`SCALE`]. Canvas painters work in design units too,
/// dividing their bounds by `SCALE` first.
pub fn px(design: f32) -> Pixels {
    gpui_kit::px(design * SCALE)
}

/// Card width; the window adds [`SHADOW`] around it.
pub const CARD_W: f32 = 440.;
/// Transparent margin around the card for its shadow.
pub const SHADOW: f32 = 16.;
/// Width of the content inside the panel: card − panel inset − padding.
pub const CONTENT_W: f32 = CARD_W - 2. - 2. * 8. - 2. - 2. * 20.;

pub fn ui_text(size: f32, color: Hsla) -> Div {
    div()
        .font_family(UI_FONT)
        .text_size(px(size))
        .text_color(color)
}

pub fn mono(size: f32, color: Hsla) -> Div {
    div()
        .font_family(MONO_FONT)
        .text_size(px(size))
        .text_color(color)
}

/// `en0 · 866 Mbps · live`: parts joined by dim dots.
pub fn dotted(parts: Vec<AnyElement>) -> Div {
    let mut row = h_flex().gap(px(4.)).overflow_hidden().whitespace_nowrap();
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            row = row.child(div().text_color(fg3()).child("·"));
        }
        row = row.child(part);
    }
    row
}

pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .text_color(color)
}

/// A 37 px square button with an icon, as in the card header.
pub fn icon_button(id: impl Into<ElementId>, child: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(37.))
        .rounded(px(10.))
        .bg(rgb(0x1c1c1c))
        .border_1()
        .border_color(rgb(0x2e2e2e))
        .flex()
        .items_center()
        .justify_center()
        .relative()
        .child(child)
}

/// Header icon with the small status dot at its lower right.
pub fn header_icon(name: &str, dot: Hsla) -> Div {
    div()
        .relative()
        .size(px(19.))
        .child(icon(name, 19., fg()))
        .child(
            div()
                .absolute()
                .right(px(-2.))
                .bottom(px(-2.))
                .size(px(7.))
                .rounded_full()
                .bg(dot)
                .border_1()
                .border_color(rgb(0x1c1c1c)),
        )
}

/// The outer card: header on top, then the inset panel.
pub fn card(header: impl IntoElement, panel_body: impl IntoElement) -> Div {
    v_flex()
        .w(px(CARD_W))
        .bg(color::card())
        .border_1()
        .border_color(card_border())
        .rounded(px(22.))
        .pb(px(8.))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.35),
            offset: point(px(0.), px(8.)),
            blur_radius: px(16.),
            spread_radius: px(0.),
            inset: false,
        }])
        .font_family(UI_FONT)
        .text_color(fg())
        .child(header)
        .child(
            v_flex()
                .mx(px(8.))
                .p(px(20.))
                .gap(px(22.))
                .bg(panel())
                .border_1()
                .border_color(panel_border())
                .rounded(px(17.))
                .child(panel_body),
        )
}

pub fn header(
    icon: impl IntoElement,
    title: impl Into<SharedString>,
    subtitle: impl IntoElement,
    chip: Option<SharedString>,
    next: Stateful<Div>,
) -> Div {
    h_flex()
        .gap(px(15.))
        .px(px(20.))
        .pt(px(20.))
        .pb(px(18.))
        .child(icon)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(
                    ui_text(17.5, fg())
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .child(ui_text(14.5, fg2()).child(subtitle)),
        )
        .children(chip.map(|c| {
            mono(13.5, fg2())
                .flex_none()
                .bg(rgb(0x141414))
                .border_1()
                .border_color(card_border())
                .rounded(px(8.))
                .px(px(10.))
                .py(px(5.))
                .child(c)
        }))
        .child(next)
}

/// A row of equal segments; `on` is the selected index.
pub fn segmented(segments: Vec<Stateful<Div>>, on: usize) -> Div {
    let count = segments.len();
    let mut row = h_flex()
        .h(px(44.))
        .p(px(3.))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .rounded(px(12.));
    for (i, seg) in segments.into_iter().enumerate() {
        let selected = i == on;
        row = row.child(
            // Many short labels: size each to its text (plus an equal share
            // of what's left), so long ones like `Overview` aren't squeezed
            // while `Fans` floats in space. A few: equal widths.
            seg.when(count >= 5, |s| s.flex_auto().px(px(4.)))
                .when(count < 5, |s| s.flex_1())
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .border_1()
                // Five or six tabs only fit at a smaller size.
                .text_size(px(match count {
                    0..=4 => 15.,
                    5 => 13.5,
                    _ => 12.5,
                }))
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(raised())
                        .border_color(raised_border())
                        .text_color(fg())
                })
                .when(!selected, |s| {
                    s.border_color(transparent_black())
                        .text_color(fg2())
                        .hover(|s| s.text_color(fg()))
                }),
        );
    }
    row
}

/// Label on the left, value on the right.
pub fn label_row(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    h_flex()
        .justify_between()
        .items_baseline()
        .child(ui_text(14.5, fg2()).child(label.into()))
        .child(value)
}

/// `27.0` big, `MB/s` small and dim, after an arrow.
pub fn rate_box(arrow: &str, rate: &str) -> Div {
    let (value, unit) = rate.split_once(' ').unwrap_or((rate, ""));
    h_flex()
        .flex_1()
        .h(px(58.))
        .justify_center()
        .gap(px(10.))
        .rounded(px(11.))
        .bg(control())
        .border_1()
        .border_color(rgb(0x252525))
        .child(icon(arrow, 16., fg4()))
        .child(mono(21., fg()).child(value.to_string()))
        .child(mono(14.5, fg3()).child(unit.to_string()))
}

/// The last two minutes as bars: download up from the midline (newest in
/// white), upload down from it.
pub fn rate_bars(bars: Vec<Option<RatePoint>>, scale: f64) -> impl IntoElement {
    const H: f32 = 92.;
    const DOWN_H: f32 = 62.;
    const UP_TOP: f32 = 66.;
    const UP_H: f32 = H - UP_TOP;
    const BAR_W: f32 = 5.5;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let n = bars.len().max(2);
            let w = bounds.size.width.as_f32() / SCALE;
            let pitch = (w - BAR_W) / (n - 1) as f32;
            let rect = |x: f32, y: f32, h: f32, color: Hsla, window: &mut Window| {
                window.paint_quad(
                    fill(
                        Bounds::new(bounds.origin + point(px(x), px(y)), size(px(BAR_W), px(h))),
                        color,
                    )
                    .corner_radii(px(1.5)),
                );
            };
            let scale = scale.max(1.0);
            for (i, bar) in bars.iter().enumerate() {
                let x = i as f32 * pitch;
                rect(x, 0., DOWN_H, track(), window);
                rect(x, UP_TOP, UP_H, track(), window);
                let Some(p) = bar else { continue };
                let last = i + 1 == bars.len();
                let dh = ((p.down / scale) as f32 * DOWN_H).clamp(3., DOWN_H);
                let uh = ((p.up / scale) as f32 * DOWN_H * 0.9).clamp(2., UP_H);
                rect(
                    x,
                    DOWN_H - dh,
                    dh,
                    if last { fg() } else { color::bar() },
                    window,
                );
                rect(x, UP_TOP, uh, if last { ok() } else { up_bar() }, window);
            }
        },
    )
    .w_full()
    .h(px(H))
}

/// Small dim caption line: left and right text.
pub fn caption(left: impl IntoElement, right: impl IntoElement) -> Div {
    h_flex()
        .justify_between()
        .text_size(px(14.))
        .text_color(fg3())
        .child(left)
        .child(right)
}

pub fn level_color(level: StorageLevel) -> Hsla {
    match level {
        StorageLevel::Ok => ok(),
        StorageLevel::Warn => caution(),
        StorageLevel::Danger => danger(),
    }
}

/// Used of total.
pub fn storage_bar(fraction: f64, color: Hsla) -> Div {
    div()
        .h(px(8.))
        .w_full()
        .rounded(px(4.))
        .bg(track())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0) as f32))
                .rounded(px(4.))
                .bg(color),
        )
}

/// The three colour zones side by side, a tick where usage is now, and the
/// two thresholds labelled under their boundaries.
pub fn zone_bar(yellow_from: f64, red_from: f64, used: f64) -> Div {
    let segment = |from: f64, to: f64, color: Hsla| {
        div()
            .h_full()
            .w(relative((to - from).max(0.0) as f32))
            .bg(color.opacity(0.85))
    };
    let label = |at: f64, text: String| {
        mono(12.5, fg3())
            .absolute()
            .top_0()
            .left(relative(at as f32))
            // Centre on the boundary (labels are about 3 characters wide).
            .ml(px(-13.))
            .child(text)
    };
    v_flex()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .h(px(16.))
                .flex()
                .items_center()
                .child(
                    h_flex()
                        .w_full()
                        .h(px(8.))
                        .rounded(px(4.))
                        .overflow_hidden()
                        .child(segment(0.0, yellow_from, ok()))
                        .child(segment(yellow_from, red_from, caution()))
                        .child(segment(red_from, 1.0, danger())),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(relative(used.clamp(0.0, 1.0) as f32))
                        .ml(px(-1.5))
                        .w(px(3.))
                        .h(px(16.))
                        .rounded(px(1.5))
                        .bg(fg()),
                ),
        )
        .child(
            div()
                .relative()
                .h(px(16.))
                .child(label(yellow_from, format!("{:.0}%", yellow_from * 100.0)))
                .child(label(red_from, format!("{:.0}%", red_from * 100.0))),
        )
}

/// Height of [`stepper`].
pub const STEPPER_H: f32 = 47.;
/// Height of [`small_button`], and of a [`stepper_sized`] that sits beside one.
pub const SMALL_H: f32 = 36.;

/// `−  50%  +`: a value with buttons either side.
pub fn stepper(
    id: impl Into<SharedString>,
    value: impl Into<SharedString>,
    minus: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    plus: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
) -> Div {
    stepper_sized(id, value, minus, plus, STEPPER_H)
}

/// A [`stepper`] `height` tall, for rows where it sits beside other controls.
pub fn stepper_sized(
    id: impl Into<SharedString>,
    value: impl Into<SharedString>,
    minus: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    plus: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    height: f32,
) -> Div {
    let id: SharedString = id.into();
    let inset = if height < STEPPER_H { 3. } else { 4. };
    let side = |suffix: &str, glyph: &str, enabled: bool| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .flex_none()
            .size(px(height - 2. * inset - 2.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(9.))
            .bg(rgb(0x1b1b1b))
            .text_size(px(20.))
            .text_color(if enabled { fg() } else { fg4() })
            .when(enabled, |b| {
                b.cursor_pointer().hover(|s| s.bg(rgb(0x242424)))
            })
            .child(glyph.to_string())
    };
    let (has_minus, has_plus) = (minus.is_some(), plus.is_some());
    let mut minus_btn = side("minus", "−", has_minus);
    if let Some(f) = minus {
        minus_btn = minus_btn.on_click(f);
    }
    let mut plus_btn = side("plus", "+", has_plus);
    if let Some(f) = plus {
        plus_btn = plus_btn.on_click(f);
    }
    h_flex()
        .flex_1()
        .h(px(height))
        .p(px(inset))
        .gap(px(6.))
        .justify_between()
        .rounded(px(if height < STEPPER_H { 10. } else { 12. }))
        .bg(control())
        .border_1()
        .border_color(rgb(0x252525))
        .child(minus_btn)
        .child(
            mono(if height < STEPPER_H { 15. } else { 16. }, fg())
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .child(value.into()),
        )
        .child(plus_btn)
}

/// Label · bar · value, for comparing values on one scale.
pub fn hbar(label: &str, fraction: f64, color: Hsla, value: impl Into<SharedString>) -> Div {
    h_flex()
        .gap(px(12.))
        .child(
            ui_text(14.5, fg2())
                .w(px(84.))
                .flex_none()
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .h(px(6.))
                .rounded(px(3.))
                .bg(track())
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction.clamp(0.0, 1.0) as f32))
                        .rounded(px(3.))
                        .bg(color),
                ),
        )
        .child(
            mono(15., fg())
                .w(px(64.))
                .flex_none()
                .flex()
                .justify_end()
                .child(value.into()),
        )
}

/// Label on the left, value on the right, in the smaller list style.
pub fn kv(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    h_flex()
        .justify_between()
        .gap(px(12.))
        .text_size(px(14.5))
        .child(div().text_color(fg2()).child(label.into()))
        .child(h_flex().gap(px(4.)).text_color(fg()).child(value))
}

/// `SESSION OPTIONS`: a small heading over a group of rows.
pub fn section_label(label: &str) -> Div {
    ui_text(13., fg3()).child(label.to_uppercase())
}

/// A compact bordered text button: `Start`, `Save`.
pub fn small_button(id: impl Into<ElementId>, label: &str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(SMALL_H))
        .min_w(px(64.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .border_1()
        .border_color(card_border())
        .bg(rgb(0x141414))
        .text_size(px(14.5))
        .text_color(if enabled { fg() } else { fg4() })
        .when(enabled, |b| {
            b.cursor_pointer().hover(|s| s.bg(rgb(0x1f1f1f)))
        })
        .child(label.to_string())
}

pub fn separator() -> Div {
    div().h(px(1.)).mx(px(-20.)).bg(sep())
}

pub fn toggle(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .relative()
        .w(px(43.))
        .h(px(25.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { ok() } else { Hsla::from(rgb(0x1c1c1c)) })
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(if on { 21. } else { 3. }))
                .size(px(19.))
                .rounded_full()
                .bg(if on { rgb(0xffffff) } else { rgb(0x3c3c3c) }),
        )
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .h(px(43.))
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(12.))
        .border_1()
        .text_size(px(15.5))
        .cursor_pointer()
        .when(primary, |b| {
            b.bg(rgb(0xededed))
                .border_color(rgb(0xededed))
                .text_color(rgb(0x111111))
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(rgb(0xffffff)))
        })
        .when(!primary, |b| {
            b.bg(rgb(0x141414))
                .border_color(card_border())
                .text_color(fg())
                .hover(|s| s.bg(rgb(0x1a1a1a)))
        })
        .child(label.into())
}
