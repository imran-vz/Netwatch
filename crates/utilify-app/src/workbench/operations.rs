//! The operations list: search, favourites, and CyberChef's categories.
//! Click an operation to append it to the recipe, or drag it to a place in
//! the recipe. Operations not ported yet are listed dimmed.

use super::Workbench;
use super::widgets::{chip, field_frame, panel};
use crate::ui::{self, color::*, px};
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use utilify_chef::Operation;
use utilify_chef::catalog::CATEGORIES;

/// An operation being dragged from the list.
#[derive(Clone)]
pub struct DraggedOp(pub &'static Operation);

/// What follows the pointer while dragging.
pub struct DragLabel(pub SharedString);

impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::ui_text(14.5, fg())
            .px(px(14.))
            .py(px(8.))
            .rounded(px(9.))
            .bg(raised())
            .border_1()
            .border_color(focus())
            .child(self.0.clone())
    }
}

impl Workbench {
    pub(super) fn operations_panel(&self, cx: &mut Context<Self>) -> Div {
        let list = if self.query.is_empty() {
            self.category_list(cx)
        } else {
            self.search_results(cx)
        };
        let ported = utilify_chef::operations().len();
        panel(
            "Operations",
            format!("{ported} ported"),
            div(),
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div().flex_none().p(px(12.)).child(
                        field_frame(false).font_family(crate::theme::UI_FONT).child(
                            Input::new(&self.search)
                                .appearance(false)
                                .text_size(px(14.5)),
                        ),
                    ),
                )
                .child(
                    v_flex()
                        .id("operations-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(8.))
                        .pb(px(8.))
                        .child(list),
                )
                .child(self.description_box()),
        )
    }

    fn category_list(&self, cx: &mut Context<Self>) -> Div {
        let mut list = v_flex().gap(px(2.));
        let favourites = self.session.favourite_ops();
        list = list.child(self.category_header("Favourites", favourites.len(), None, cx));
        if self.expanded.contains("Favourites") {
            list = list.children(favourites.iter().map(|op| self.op_row("fav", op, cx)));
        }
        for &(name, names) in CATEGORIES {
            let ops: Vec<&'static Operation> =
                names.iter().filter_map(|n| utilify_chef::find(n)).collect();
            list = list.child(self.category_header(name, ops.len(), Some(names.len()), cx));
            if self.expanded.contains(name) {
                for n in names {
                    list = list.child(match utilify_chef::find(n) {
                        Some(op) => self.op_row(name, op, cx).into_any_element(),
                        None => unported_row(n).into_any_element(),
                    });
                }
            }
        }
        list
    }

    /// Name and description matches, names first; then catalog names not
    /// ported yet.
    fn search_results(&self, cx: &mut Context<Self>) -> Div {
        let q = self.query.as_str();
        let ops = utilify_chef::operations();
        let by_name = ops.iter().filter(|op| op.name.to_lowercase().contains(q));
        let by_desc = ops.iter().filter(|op| {
            !op.name.to_lowercase().contains(q) && op.description.to_lowercase().contains(q)
        });
        let rows: Vec<_> = by_name
            .chain(by_desc)
            .map(|op| self.op_row("search", op, cx))
            .collect();
        let pending: Vec<&str> = CATEGORIES
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .filter(|n| utilify_chef::find(n).is_none() && n.to_lowercase().contains(q))
            .collect();
        let empty = rows.is_empty() && pending.is_empty();
        v_flex()
            .gap(px(2.))
            .children(rows)
            .children(pending.into_iter().map(unported_row))
            .when(empty, |v| {
                v.child(
                    ui::ui_text(14.5, fg3())
                        .p(px(12.))
                        .child("No operations match"),
                )
            })
    }

    fn category_header(
        &self,
        name: &'static str,
        ported: usize,
        total: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.expanded.contains(name);
        let count = match total {
            Some(t) if ported < t => format!("{ported}/{t}"),
            _ => ported.to_string(),
        };
        let available = ported > 0 || total.is_none();
        h_flex()
            .id(SharedString::from(format!("cat-{name}")))
            .h(px(36.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x151515)))
            .child(ui::icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                13.,
                fg3(),
            ))
            .child(
                ui::ui_text(14.5, if available { fg() } else { fg3() })
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(ui::mono(12.5, fg3()).child(count))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(name) {
                    this.expanded.insert(name);
                }
                cx.notify();
            }))
    }

    fn op_row(
        &self,
        section: &str,
        op: &'static Operation,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let favourite = self.session.is_favourite(op.name);
        let hovered = self.hovered.is_some_and(|h| std::ptr::eq(h, op));
        h_flex()
            .id(SharedString::from(format!("op-{section}-{}", op.name)))
            .h(px(34.))
            .pl(px(29.))
            .pr(px(6.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_size(px(14.5))
            .text_color(fg2())
            .cursor_pointer()
            .hover(|s| s.bg(raised()).text_color(fg()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(op.name),
            )
            .child(
                div()
                    .id(SharedString::from(format!("fav-{section}-{}", op.name)))
                    .flex_none()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .hover(|s| s.bg(rgb(0x2a2a2a)))
                    .when(favourite || hovered, |d| {
                        d.child(ui::icon(
                            "star",
                            14.,
                            if favourite { caution() } else { fg4() },
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.session.toggle_favourite(op.name);
                        this.save_soon(cx);
                        cx.notify();
                    })),
            )
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                if *over {
                    this.hovered = Some(op);
                } else if this.hovered.is_some_and(|h| std::ptr::eq(h, op)) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.add_op(op, None, window, cx)))
            .on_drag(DraggedOp(op), |d, _, _, cx| {
                cx.new(|_| DragLabel(d.0.name.into()))
            })
    }

    /// The hovered operation's description, or how to use the list.
    fn description_box(&self) -> Div {
        let (title, body): (SharedString, SharedString) = match self.hovered {
            Some(op) => (op.name.into(), op.description.into()),
            None => (
                "Add operations".into(),
                "Click an operation to add it to the end of the recipe, or drag it into place. Star the ones you use most.".into(),
            ),
        };
        v_flex()
            .flex_none()
            .h(px(150.))
            .mx(px(12.))
            .mb(px(12.))
            .p(px(14.))
            .gap(px(6.))
            .rounded(px(12.))
            .bg(control())
            .border_1()
            .border_color(rgb(0x252525))
            .overflow_hidden()
            .child(
                ui::ui_text(14., fg())
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(ui::ui_text(13., fg3()).line_height(px(19.)).child(body))
    }
}

fn unported_row(name: &str) -> Div {
    h_flex()
        .h(px(34.))
        .pl(px(29.))
        .pr(px(10.))
        .gap(px(8.))
        .text_size(px(14.5))
        .text_color(fg4())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(name.to_string()),
        )
        .child(chip("soon", fg4()))
}
