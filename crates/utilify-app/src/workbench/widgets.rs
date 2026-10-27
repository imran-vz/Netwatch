//! Pieces the workbench's panels are built from, in the dashboard's style
//! (see [`crate::ui`]): inset panels, small icon buttons and a dropdown.

use crate::ui::{
    self,
    color::{self, *},
    px,
};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// An inset panel with a title row (title, caption, actions) over its body.
pub fn panel(
    title: &str,
    caption: impl IntoElement,
    actions: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    v_flex()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .bg(color::panel())
        .border_1()
        .border_color(panel_border())
        .rounded(px(17.))
        .child(
            h_flex()
                .flex_none()
                .h(px(56.))
                .px(px(18.))
                .gap(px(10.))
                .border_b_1()
                .border_color(sep())
                .child(
                    ui::ui_text(15.5, fg())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_none()
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(13.5))
                        .text_color(fg3())
                        .child(caption),
                )
                .child(h_flex().flex_none().gap(px(6.)).child(actions)),
        )
        .child(body)
}

/// A 30 px square button with an icon, for panel title rows and step cards.
pub fn tool_button(id: impl Into<ElementId>, icon: &str, color: Hsla) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(30.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(raised()))
        .child(ui::icon(icon, 15., color))
}

/// A small text button: `Copy`, `Paste`…
pub fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(30.))
        .px(px(11.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .border_1()
        .border_color(card_border())
        .bg(rgb(0x141414))
        .text_size(px(13.5))
        .text_color(fg2())
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x1a1a1a)).text_color(fg()))
        .child(label.into())
}

/// A small coloured label: `binary`, `paused`, `soon`.
pub fn chip(label: impl Into<SharedString>, color: Hsla) -> Div {
    ui::mono(12., color)
        .flex_none()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(color.opacity(0.12))
        .child(label.into())
}

/// What picking a dropdown entry does.
pub type PickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A dropdown entry.
pub struct Choice {
    pub label: SharedString,
    pub selected: bool,
    pub on_pick: PickHandler,
}

/// A pill showing `current`; when `open`, a menu of `choices` under it.
/// `on_toggle` flips it open, `on_dismiss` closes it on a click elsewhere.
pub fn dropdown(
    id: SharedString,
    current: impl Into<SharedString>,
    open: bool,
    choices: Vec<Choice>,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let pill = h_flex()
        .id(id.clone())
        .h(px(38.))
        .w_full()
        .px(px(12.))
        .gap(px(8.))
        .justify_between()
        .rounded(px(10.))
        .bg(control())
        .border_1()
        .border_color(if open { focus() } else { control_border() })
        .text_size(px(14.5))
        .text_color(fg())
        .cursor_pointer()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(current.into()),
        )
        .child(ui::icon(
            if open { "chevron-up" } else { "chevron-down" },
            12.,
            fg2(),
        ))
        .on_click(on_toggle);

    let menu = open.then(|| {
        let items = choices.into_iter().enumerate().map(|(i, c)| {
            h_flex()
                .id(i)
                .min_h(px(32.))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(7.))
                .text_size(px(14.))
                .text_color(if c.selected { fg() } else { fg2() })
                .cursor_pointer()
                .hover(|s| s.bg(raised()).text_color(fg()))
                .child(div().flex_1().min_w_0().child(c.label))
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .children(c.selected.then(|| ui::icon("check", 14., ok()))),
                )
                .on_click(c.on_pick)
        });
        deferred(
            v_flex()
                .id(SharedString::from(format!("{id}-menu")))
                .absolute()
                .top(px(42.))
                .left_0()
                .right_0()
                .min_w(px(180.))
                .max_h(px(420.))
                .overflow_y_scroll()
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
        .w_full()
        .child(pill)
        .children(menu)
        .when(open, |d| d.on_mouse_down_out(on_dismiss))
}

/// The frame a text field sits in, matching the dropdown pill.
pub fn field_frame(focused: bool) -> Div {
    div()
        .w_full()
        .min_h(px(38.))
        .px(px(4.))
        .flex()
        .items_center()
        .rounded(px(10.))
        .bg(control())
        .border_1()
        .border_color(if focused { focus() } else { control_border() })
        .font_family(crate::theme::MONO_FONT)
        .text_size(px(14.))
        .text_color(fg())
}
