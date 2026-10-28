//! The visual language shared by the dashboard and the workbench: semantic
//! colours for dark and light appearance, the type and corner scales, and the
//! building blocks the tabs and panels are made of.
//!
//! Lengths are written at design scale and go through [`px`], which applies
//! [`SCALE`]. Corner radii are the exception: they're absolute ([`radius`]),
//! so corners stay crisp (3–6 px) whatever the scale.

use crate::theme::{MONO_FONT, UI_FONT};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use utilify_core::history::RatePoint;
use utilify_core::settings::StorageLevel;

pub mod color {
    //! Semantic colours. Each token has a dark and a light value; which one
    //! applies follows the OS appearance ([`set_light`]).

    use gpui_kit::{Hsla, rgb};
    use std::sync::atomic::{AtomicBool, Ordering};

    static LIGHT: AtomicBool = AtomicBool::new(false);

    /// Switch every token to the light (or dark) palette. Views re-render
    /// on appearance changes, so this takes effect on the next frame.
    pub fn set_light(light: bool) {
        LIGHT.store(light, Ordering::Relaxed);
    }

    pub fn is_light() -> bool {
        LIGHT.load(Ordering::Relaxed)
    }

    fn pick(dark: u32, light: u32) -> Hsla {
        rgb(if is_light() { light } else { dark }).into()
    }

    /// The window surface: header, tab bar and the frame round the panel.
    pub fn card() -> Hsla {
        pick(0x171717, 0xf4f4f4)
    }
    pub fn card_border() -> Hsla {
        pick(0x2c2c2c, 0xd6d6d6)
    }
    /// The inset content panel.
    pub fn panel() -> Hsla {
        pick(0x0f0f0f, 0xffffff)
    }
    pub fn panel_border() -> Hsla {
        pick(0x232323, 0xe2e2e2)
    }
    /// Wells: segmented tracks, steppers, fields, rate boxes.
    pub fn control() -> Hsla {
        pick(0x141414, 0xf6f6f6)
    }
    pub fn control_border() -> Hsla {
        pick(0x2a2a2a, 0xdcdcdc)
    }
    pub fn control_hover() -> Hsla {
        pick(0x1d1d1d, 0xececec)
    }
    /// Buttons and the selected segment, a step above the well.
    pub fn raised() -> Hsla {
        pick(0x222222, 0xffffff)
    }
    pub fn raised_border() -> Hsla {
        pick(0x363636, 0xcccccc)
    }
    pub fn raised_hover() -> Hsla {
        pick(0x2a2a2a, 0xf0f0f0)
    }
    /// Menus, tooltips and toasts float above everything.
    pub fn popover() -> Hsla {
        pick(0x1b1b1b, 0xffffff)
    }
    pub fn sep() -> Hsla {
        pick(0x232323, 0xe8e8e8)
    }
    /// Primary text.
    pub fn fg() -> Hsla {
        pick(0xf2f2f2, 0x161616)
    }
    /// Secondary text: labels, values that matter less.
    pub fn fg2() -> Hsla {
        pick(0xb4b4b4, 0x4a4a4a)
    }
    /// Tertiary text: captions and details. Still 4.5:1 on the panel.
    pub fn fg3() -> Hsla {
        pick(0x8d8d8d, 0x696969)
    }
    /// Disabled text and decoration only (3:1); never for reading.
    pub fn fg4() -> Hsla {
        pick(0x686868, 0x969696)
    }
    pub fn bar() -> Hsla {
        pick(0x5c5c5c, 0xbcbcbc)
    }
    pub fn bar_lite() -> Hsla {
        pick(0xd8d8d8, 0x3a3a3a)
    }
    pub fn track() -> Hsla {
        pick(0x1f1f1f, 0xebebeb)
    }
    pub fn up_bar() -> Hsla {
        pick(0x3a7a52, 0x9ed6b3)
    }
    pub fn ok() -> Hsla {
        pick(0x4fcf7d, 0x17864a)
    }
    pub fn caution() -> Hsla {
        pick(0xe8b54a, 0x9a6a00)
    }
    pub fn danger() -> Hsla {
        pick(0xf0675c, 0xcf3328)
    }
    pub fn info() -> Hsla {
        pick(0x7ab0ff, 0x2a66cc)
    }
    /// Focus and "open" outlines.
    pub fn focus() -> Hsla {
        pick(0x3e9e66, 0x2f9a5c)
    }
    /// The storage chart's plot area and its grid.
    pub fn chart() -> Hsla {
        pick(0x0c0c0c, 0xfbfbfb)
    }
    pub fn grid() -> Hsla {
        pick(0x151515, 0xf1f1f1)
    }
    pub fn grid_strong() -> Hsla {
        pick(0x1c1c1c, 0xe5e5e5)
    }
    pub fn guide() -> Hsla {
        pick(0x5a5a5a, 0xb4b4b4)
    }
    pub fn hairline() -> Hsla {
        pick(0x3a3a3a, 0xc8c8c8)
    }
    /// The primary button: inverted against the surface.
    pub fn primary() -> Hsla {
        pick(0xededed, 0x1b1b1b)
    }
    pub fn primary_hover() -> Hsla {
        pick(0xffffff, 0x000000)
    }
    pub fn on_primary() -> Hsla {
        pick(0x111111, 0xffffff)
    }
    pub fn toggle_off() -> Hsla {
        pick(0x262626, 0xd9d9d9)
    }
    pub fn knob_off() -> Hsla {
        pick(0x5e5e5e, 0xffffff)
    }
    /// The knob on a switched-on toggle.
    pub fn knob_on() -> Hsla {
        rgb(0xffffff).into()
    }
    pub fn shadow() -> Hsla {
        if is_light() {
            gpui_kit::hsla(0., 0., 0., 0.14)
        } else {
            gpui_kit::hsla(0., 0., 0., 0.45)
        }
    }
}

use color::*;

/// Overall size of the UI. Every length is written at design scale (the
/// mock's numbers) and goes through [`px`], which applies this.
pub const SCALE: f32 = 0.77;

/// Design-scale pixels: shadows `gpui_kit::px` here and in the views so the
/// whole UI follows [`SCALE`]. Canvas painters work in design units too,
/// dividing their bounds by `SCALE` first.
pub fn px(design: f32) -> Pixels {
    gpui_kit::px(design * SCALE)
}

/// Corner radii, in real pixels (not scaled): small parts (chips, bars,
/// segments inside a track), controls and panels, and the window card.
pub const R_SM: f32 = 3.;
pub const R_MD: f32 = 4.;
pub const R_LG: f32 = 6.;

pub fn radius(r: f32) -> Pixels {
    gpui_kit::px(r)
}

/// The type scale, in design units. Nothing reads smaller than
/// [`text::MICRO`] (about 10 px on screen).
pub mod text {
    pub const MICRO: f32 = 13.;
    pub const CAPTION: f32 = 14.;
    pub const SMALL: f32 = 15.;
    pub const BODY: f32 = 16.;
    pub const LABEL: f32 = 17.;
    pub const TITLE: f32 = 19.;
    pub const VALUE: f32 = 22.;
}

/// Card width; the window adds [`SHADOW`] around it.
pub const CARD_W: f32 = 440.;
/// Transparent margin around the card for its shadow.
pub const SHADOW: f32 = 16.;
/// Width of the content inside the panel: card − panel inset − padding.
pub const CONTENT_W: f32 = CARD_W - 2. - 2. * 8. - 2. - 2. * 20.;

fn drop_shadow(y: f32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: shadow(),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

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
    let mut row = h_flex().gap(px(5.)).overflow_hidden().whitespace_nowrap();
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            row = row.child(div().text_color(fg4()).child("·"));
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

/// A tooltip: a short label (and an optional shortcut) on a floating chip.
pub struct Tip {
    label: SharedString,
    keys: Option<SharedString>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap(px(8.))
            .px(px(9.))
            .py(px(5.))
            .rounded(radius(R_MD))
            .bg(popover())
            .border_1()
            .border_color(raised_border())
            .shadow(drop_shadow(4., 12.))
            .font_family(UI_FONT)
            .text_size(px(text::CAPTION))
            .text_color(fg())
            .child(self.label.clone())
            .children(
                self.keys
                    .clone()
                    .map(|k| mono(text::MICRO, fg3()).child(k)),
            )
    }
}

/// For `.tooltip(...)`: `tip("Open Workbench", Some("⌘B"))`.
pub fn tip(
    label: impl Into<SharedString>,
    keys: Option<&str>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label = label.into();
    let keys: Option<SharedString> = keys.map(|k| SharedString::from(k.to_string()));
    move |_, cx| {
        let (label, keys) = (label.clone(), keys.clone());
        cx.new(|_| Tip { label, keys }).into()
    }
}

/// A 37 px square button with an icon, as in the card header. Add a
/// [`tip`]: an icon alone doesn't say what it does.
pub fn icon_button(id: impl Into<ElementId>, child: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(37.))
        .rounded(radius(R_MD))
        .bg(raised())
        .border_1()
        .border_color(raised_border())
        .flex()
        .items_center()
        .justify_center()
        .relative()
        .child(child)
}

/// An [`icon_button`] that does something: hover, pointer and tooltip.
pub fn icon_action(
    id: impl Into<ElementId>,
    name: &str,
    label: &str,
    keys: Option<&str>,
) -> Stateful<Div> {
    icon_button(id, icon(name, 18., fg()))
        .cursor_pointer()
        .hover(|s| s.bg(raised_hover()))
        .tooltip(tip(label.to_string(), keys))
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
                .right(px(-3.))
                .bottom(px(-3.))
                .size(px(8.))
                .rounded_full()
                .bg(dot)
                .border_1()
                .border_color(raised()),
        )
}

/// The outer card: header and tab bar on the card surface, then the inset
/// panel with the tab's content.
pub fn card(header: impl IntoElement, tabs: impl IntoElement, panel_body: impl IntoElement) -> Div {
    v_flex()
        .w(px(CARD_W))
        .bg(color::card())
        .border_1()
        .border_color(card_border())
        .rounded(radius(R_LG))
        .pb(px(8.))
        .shadow(drop_shadow(10., 24.))
        .font_family(UI_FONT)
        .text_color(fg())
        .child(header)
        .child(div().px(px(8.)).pb(px(8.)).child(tabs))
        .child(
            v_flex()
                .mx(px(8.))
                .bg(panel())
                .border_1()
                .border_color(panel_border())
                .rounded(radius(R_MD))
                .overflow_hidden()
                .child(panel_body),
        )
}

pub fn header(
    icon: impl IntoElement,
    title: impl Into<SharedString>,
    subtitle: impl IntoElement,
    chip: Option<SharedString>,
    actions: impl IntoElement,
) -> Div {
    h_flex()
        .gap(px(14.))
        .px(px(18.))
        .pt(px(18.))
        .pb(px(14.))
        .child(icon)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(
                    ui_text(text::TITLE, fg())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(ui_text(text::SMALL, fg2()).child(subtitle)),
        )
        .children(chip.map(|c| {
            mono(text::CAPTION, fg2())
                .flex_none()
                .bg(control())
                .border_1()
                .border_color(control_border())
                .rounded(radius(R_SM))
                .px(px(9.))
                .py(px(4.))
                .child(c)
        }))
        .child(actions)
}

/// A click handler, as tabs and toasts store them.
pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// Something to run later with the window, like a toast's Undo.
pub type Callback = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// One entry in [`tab_bar`].
pub struct TabItem {
    pub id: SharedString,
    pub icon: &'static str,
    pub label: &'static str,
    /// Shown in the tooltip, like `⌘1`.
    pub keys: SharedString,
    /// A small dot on the icon: something on this tab wants attention.
    pub badge: Option<Hsla>,
    pub on_click: ClickHandler,
}

/// The dashboard's tabs: an icon over a label in equal columns, the current
/// one raised.
pub fn tab_bar(items: Vec<TabItem>, on: usize) -> Div {
    let mut row = h_flex()
        .p(px(3.))
        .gap(px(2.))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .rounded(radius(R_MD));
    for (i, item) in items.into_iter().enumerate() {
        let selected = i == on;
        let tint = if selected { fg() } else { fg3() };
        row = row.child(
            v_flex()
                .id(item.id)
                .flex_1()
                .min_w_0()
                .h(px(54.))
                .items_center()
                .justify_center()
                .gap(px(4.))
                .rounded(radius(R_SM))
                .border_1()
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(raised())
                        .border_color(raised_border())
                        .shadow(drop_shadow(1., 2.))
                })
                .when(!selected, |s| {
                    s.border_color(transparent_black())
                        .hover(|s| s.bg(control_hover()))
                })
                .tooltip(tip(item.label, Some(item.keys.as_ref())))
                .on_click(item.on_click)
                .child(
                    div()
                        .relative()
                        .child(icon(item.icon, 17., tint))
                        .children(item.badge.map(|c| {
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-4.))
                                .size(px(7.))
                                .rounded_full()
                                .bg(c)
                        })),
                )
                .child(
                    ui_text(text::MICRO, if selected { fg() } else { fg2() })
                        .when(selected, |t| t.font_weight(FontWeight::MEDIUM))
                        .whitespace_nowrap()
                        .child(item.label),
                ),
        );
    }
    row
}

/// A row of equal segments; `on` is the selected index.
pub fn segmented(segments: Vec<Stateful<Div>>, on: usize) -> Div {
    let mut row = h_flex()
        .h(px(42.))
        .p(px(3.))
        .gap(px(2.))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .rounded(radius(R_MD));
    for (i, seg) in segments.into_iter().enumerate() {
        let selected = i == on;
        row = row.child(
            seg.flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius(R_SM))
                .border_1()
                .text_size(px(text::SMALL))
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(raised())
                        .border_color(raised_border())
                        .text_color(fg())
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |s| {
                    s.border_color(transparent_black())
                        .text_color(fg2())
                        .hover(|s| s.bg(control_hover()).text_color(fg()))
                }),
        );
    }
    row
}

/// A titled group of rows: `NETWORK` with an optional note on the right.
pub fn section(title: &str, note: Option<AnyElement>, body: impl IntoElement) -> Div {
    v_flex()
        .gap(px(10.))
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .gap(px(10.))
                .child(section_label(title))
                .children(note),
        )
        .child(body)
}

/// Label on the left, value on the right.
pub fn label_row(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    h_flex()
        .justify_between()
        .items_baseline()
        .gap(px(12.))
        .child(ui_text(text::BODY, fg2()).child(label.into()))
        .child(value)
}

/// `27.0` big, `MB/s` small and dim, after an arrow.
pub fn rate_box(arrow: &str, label: &str, rate: &str) -> Div {
    let (value, unit) = rate.split_once(' ').unwrap_or((rate, ""));
    v_flex()
        .flex_1()
        .gap(px(4.))
        .px(px(14.))
        .py(px(11.))
        .rounded(radius(R_MD))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .child(
            h_flex()
                .gap(px(6.))
                .child(icon(arrow, 14., fg3()))
                .child(ui_text(text::CAPTION, fg3()).child(label.to_string())),
        )
        .child(
            h_flex()
                .items_baseline()
                .gap(px(7.))
                .child(mono(text::VALUE, fg()).child(value.to_string()))
                .child(mono(text::SMALL, fg3()).child(unit.to_string())),
        )
}

/// The last two minutes as bars: download up from the midline (newest in
/// full colour), upload down from it.
pub fn rate_bars(bars: Vec<Option<RatePoint>>, scale: f64) -> impl IntoElement {
    const H: f32 = 96.;
    const DOWN_H: f32 = 64.;
    const UP_TOP: f32 = 68.;
    const UP_H: f32 = H - UP_TOP;
    const BAR_W: f32 = 5.5;
    let (newest_down, newest_up, down, up, empty) = (fg(), ok(), bar(), up_bar(), track());
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
                    .corner_radii(gpui_kit::px(1.)),
                );
            };
            let scale = scale.max(1.0);
            for (i, b) in bars.iter().enumerate() {
                let x = i as f32 * pitch;
                rect(x, 0., DOWN_H, empty, window);
                rect(x, UP_TOP, UP_H, empty, window);
                let Some(p) = b else { continue };
                let last = i + 1 == bars.len();
                let dh = ((p.down / scale) as f32 * DOWN_H).clamp(3., DOWN_H);
                let uh = ((p.up / scale) as f32 * DOWN_H * 0.9).clamp(2., UP_H);
                rect(x, DOWN_H - dh, dh, if last { newest_down } else { down }, window);
                rect(x, UP_TOP, uh, if last { newest_up } else { up }, window);
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
        .gap(px(12.))
        .text_size(px(text::CAPTION))
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
    meter(fraction, color, 8.)
}

/// A filled bar `height` tall on the track.
pub fn meter(fraction: f64, color: Hsla, height: f32) -> Div {
    div()
        .h(px(height))
        .w_full()
        .rounded(radius(R_SM))
        .bg(track())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0) as f32))
                .rounded(radius(R_SM))
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
        mono(text::MICRO, fg3())
            .absolute()
            .top_0()
            .left(relative(at as f32))
            // Centre on the boundary (labels are about 3 characters wide).
            .ml(px(-14.))
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
                        .rounded(radius(R_SM))
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
                        .rounded(gpui_kit::px(1.))
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
pub const STEPPER_H: f32 = 44.;
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
    let inset = 3.;
    let side = |suffix: &str, glyph: &str, label: &str, enabled: bool| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .flex_none()
            .size(px(height - 2. * inset - 2.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius(R_SM))
            .bg(raised())
            .border_1()
            .border_color(raised_border())
            .child(icon(glyph, 14., if enabled { fg() } else { fg4() }))
            .when(enabled, |b| {
                b.cursor_pointer()
                    .hover(|s| s.bg(raised_hover()))
                    .tooltip(tip(label.to_string(), None))
            })
            .when(!enabled, |b| b.opacity(0.6))
    };
    let (has_minus, has_plus) = (minus.is_some(), plus.is_some());
    let mut minus_btn = side("minus", "minus", "Less", has_minus);
    if let Some(f) = minus {
        minus_btn = minus_btn.on_click(f);
    }
    let mut plus_btn = side("plus", "plus", "More", has_plus);
    if let Some(f) = plus {
        plus_btn = plus_btn.on_click(f);
    }
    h_flex()
        .flex_1()
        .h(px(height))
        .p(px(inset))
        .gap(px(6.))
        .justify_between()
        .rounded(radius(R_MD))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .child(minus_btn)
        .child(
            mono(text::BODY, fg())
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
            ui_text(text::SMALL, fg2())
                .w(px(88.))
                .flex_none()
                .child(label.to_string()),
        )
        .child(div().flex_1().child(meter(fraction, color, 6.)))
        .child(
            mono(text::SMALL, fg())
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
        .min_h(px(24.))
        .text_size(px(text::SMALL))
        .child(div().text_color(fg2()).child(label.into()))
        .child(h_flex().gap(px(4.)).text_color(fg()).child(value))
}

/// `SESSION OPTIONS`: a small heading over a group of rows.
pub fn section_label(label: &str) -> Div {
    ui_text(text::MICRO, fg3())
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_uppercase())
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
        .gap(px(6.))
        .rounded(radius(R_MD))
        .border_1()
        .border_color(raised_border())
        .bg(raised())
        .text_size(px(text::SMALL))
        .text_color(if enabled { fg() } else { fg4() })
        .when(enabled, |b| {
            b.cursor_pointer().hover(|s| s.bg(raised_hover()))
        })
        .child(label.to_string())
}

pub fn separator() -> Div {
    div().h(px(1.)).mx(px(-20.)).bg(sep())
}

/// An on/off switch. `on_click` flips it; without one it reads as disabled.
pub fn toggle(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .relative()
        .w(px(42.))
        .h(px(24.))
        .rounded(radius(R_MD))
        .cursor_pointer()
        .border_1()
        .border_color(if on { ok() } else { control_border() })
        .bg(if on { ok() } else { toggle_off() })
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 20. } else { 2. }))
                .size(px(18.))
                .rounded(radius(R_SM))
                .bg(if on { knob_on() } else { knob_off() })
                .shadow(drop_shadow(1., 2.)),
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
        .h(px(42.))
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .rounded(radius(R_MD))
        .border_1()
        .text_size(px(text::BODY))
        .cursor_pointer()
        .when(primary, |b| {
            b.bg(color::primary())
                .border_color(color::primary())
                .text_color(on_primary())
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(primary_hover()))
        })
        .when(!primary, |b| {
            b.bg(raised())
                .border_color(raised_border())
                .text_color(fg())
                .hover(|s| s.bg(raised_hover()))
        })
        .child(label.into())
}

/// A [`button`] for things that can't be taken back, in the danger colour.
pub fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(id, label, false)
        .bg(danger().opacity(0.12))
        .border_color(danger().opacity(0.5))
        .text_color(danger())
        .hover(|s| s.bg(danger().opacity(0.2)))
}

/// How serious a [`banner`] or [`toast`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Ok,
    Caution,
    Danger,
}

impl Tone {
    pub fn color(self) -> Hsla {
        match self {
            Tone::Info => info(),
            Tone::Ok => ok(),
            Tone::Caution => caution(),
            Tone::Danger => danger(),
        }
    }

    /// Colour never carries the meaning alone: each tone has its icon.
    pub fn icon(self) -> &'static str {
        match self {
            Tone::Info => "info",
            Tone::Ok => "circle-check",
            Tone::Caution => "triangle-alert",
            Tone::Danger => "circle-alert",
        }
    }
}

/// A boxed note: an icon, a title, lines under it and, optionally, buttons
/// that fix the problem.
pub fn banner(
    tone: Tone,
    title: impl Into<SharedString>,
    lines: Vec<AnyElement>,
    actions: Vec<Stateful<Div>>,
) -> Div {
    let color = tone.color();
    h_flex()
        .items_start()
        .gap(px(11.))
        .p(px(13.))
        .rounded(radius(R_MD))
        .border_1()
        .border_color(color.opacity(0.4))
        .bg(color.opacity(0.08))
        .child(div().pt(px(2.)).child(icon(tone.icon(), 16., color)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(
                    ui_text(text::BODY, fg())
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .children(lines)
                .when(!actions.is_empty(), |v| {
                    v.child(
                        h_flex()
                            .pt(px(4.))
                            .gap(px(8.))
                            .flex_wrap()
                            .children(actions),
                    )
                }),
        )
}

/// A line of text in a [`banner`].
pub fn note(s: impl Into<SharedString>) -> AnyElement {
    ui_text(text::SMALL, fg2()).child(s.into()).into_any_element()
}

/// A small button for a [`banner`]'s actions row.
pub fn banner_button(id: impl Into<ElementId>, label: &str) -> Stateful<Div> {
    small_button(id, label, true).h(px(32.)).min_w(px(0.))
}

/// A command to paste into Terminal, with a copy button beside it.
/// `on_copy` runs after it's on the clipboard (to confirm it).
pub fn command(
    id: impl Into<SharedString>,
    cmd: &str,
    on_copy: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    let text = cmd.to_string();
    h_flex()
        .gap(px(6.))
        .pl(px(10.))
        .pr(px(4.))
        .py(px(4.))
        .rounded(radius(R_SM))
        .bg(chart())
        .border_1()
        .border_color(control_border())
        .child(
            mono(text::CAPTION, fg())
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(cmd.to_string()),
        )
        .child(
            div()
                .id(id)
                .flex_none()
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius(R_SM))
                .cursor_pointer()
                .hover(|s| s.bg(control_hover()))
                .tooltip(tip("Copy command", None))
                .child(icon("copy", 14., fg2()))
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    on_copy(window, cx);
                }),
        )
        .into_any_element()
}

/// A transient confirmation along the card's bottom edge, with an optional
/// action (Undo) on the right.
pub fn toast(tone: Tone, message: SharedString, action: Option<Stateful<Div>>) -> Div {
    h_flex()
        .gap(px(10.))
        .pl(px(13.))
        .pr(px(6.))
        .min_h(px(42.))
        .rounded(radius(R_MD))
        .bg(popover())
        .border_1()
        .border_color(raised_border())
        .shadow(drop_shadow(8., 24.))
        .child(icon(tone.icon(), 15., tone.color()))
        .child(
            ui_text(text::SMALL, fg())
                .flex_1()
                .min_w_0()
                .py(px(8.))
                .child(message),
        )
        .children(action)
}

/// A small inline status pill: `live`, `stale`, `on battery`.
pub fn pill(label: impl Into<SharedString>, color: Hsla) -> Div {
    ui_text(text::MICRO, color)
        .flex_none()
        .px(px(7.))
        .py(px(2.))
        .rounded(radius(R_SM))
        .bg(color.opacity(0.12))
        .child(label.into())
}

/// An empty or loading state: an icon, a line saying what's going on, and
/// a dimmer one saying what happens next.
pub fn empty_state(icon_name: &str, title: &str, detail: &str) -> Div {
    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(18.))
        .child(icon(icon_name, 20., fg3()))
        .child(ui_text(text::BODY, fg2()).child(title.to_string()))
        .child(
            ui_text(text::CAPTION, fg3())
                .text_center()
                .child(detail.to_string()),
        )
}
