//! The workbench: a full window for chaining data operations into recipes,
//! like CyberChef. Operations on the left ([`operations`]), the recipe in
//! the middle ([`recipe`]), input over output on the right ([`io`]). The
//! operations come from `utilify-chef`; the look is the dashboard's.
//!
//! While it's open Utilify shows in the Dock and the app switcher, and gets
//! a menu bar; closing it goes back to menu-bar only.

mod io;
mod operations;
mod recipe;
mod widgets;

use crate::theme::UI_FONT;
use crate::ui::{
    self,
    color::{self, *},
    px,
};
use gpui_kit::component::input::{
    Copy, Cut, InputState, Paste, Redo, SelectAll, TextareaState, Undo,
};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use utilify_chef::recipe::Halt;
use utilify_chef::session::{self, Session};
use utilify_chef::{Dish, Operation, Recipe};

use recipe::StepView;

actions!(workbench, [Bake, CloseWorkbench, QuitApp]);

const KEY_CONTEXT: &str = "Workbench";
/// Pause after an edit before auto bake runs, so typing doesn't bake on
/// every keystroke of a long input.
const BAKE_DELAY: Duration = Duration::from_millis(120);
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// The output pane shows at most this much; Copy and Save take all of it.
const MAX_SHOWN: usize = 1024 * 1024;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", Bake, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-w", CloseWorkbench, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-q", QuitApp, Some(KEY_CONTEXT)),
    ]);
    cx.on_action(|_: &QuitApp, cx| cx.quit());
}

/// Raw bytes loaded in place of typed input: an opened file, or binary
/// output moved to the input.
#[derive(Clone)]
struct InputFile {
    name: SharedString,
    bytes: std::sync::Arc<Vec<u8>>,
}

/// The last bake, as the output pane shows it.
struct Output {
    dish: Dish,
    halt: Option<Halt>,
    duration: Duration,
    chars: usize,
    lines: usize,
    truncated: bool,
}

pub struct Workbench {
    session: Session,
    session_path: Option<PathBuf>,
    focus: FocusHandle,

    search: Entity<InputState>,
    query: String,
    /// Categories unfolded in the operations list.
    expanded: HashSet<&'static str>,
    hovered: Option<&'static Operation>,

    steps: Vec<StepView>,
    next_step_id: u64,
    /// The dropdown that's open: a step's id and the argument's index.
    menu: Option<(u64, usize)>,

    input: Entity<TextareaState>,
    input_file: Option<InputFile>,
    output: Entity<TextareaState>,
    result: Option<Output>,
    baking: bool,
    /// Bumped by every bake request; only the latest one's result is shown.
    generation: u64,
    save_task: Option<Task<()>>,
    /// A short confirmation in the header, like "Recipe copied".
    notice: Option<SharedString>,
    notice_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session_path = session_path();
        let session = session_path
            .as_deref()
            .map(session::load)
            .unwrap_or_default();

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search operations"));
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Type or paste input, or drop a file here")
                .default_value(session.input.clone())
        });
        let output = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true));

        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, state, ev, _, cx| {
                if matches!(ev, gpui_kit::component::input::InputEvent::Change) {
                    this.query = state.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
                gpui_kit::component::input::InputEvent::Change => {
                    this.input_changed(window, cx);
                }
                gpui_kit::component::input::InputEvent::PressEnter {
                    secondary: true, ..
                } => {
                    this.bake(None, window, cx);
                }
                _ => {}
            }),
            cx.on_release(|this, cx| {
                this.store_session(cx);
                if let Some(path) = &this.session_path
                    && let Err(e) = session::save(path, &this.session)
                {
                    log::warn!("Saving the workbench failed: {e}");
                }
                closed(cx);
            }),
        ];

        let mut this = Self {
            expanded: ["Favourites", utilify_chef::ops::data_format::CATEGORY]
                .into_iter()
                .collect(),
            session,
            session_path,
            focus: cx.focus_handle(),
            search,
            query: String::new(),
            hovered: None,
            steps: vec![],
            next_step_id: 0,
            menu: None,
            input,
            input_file: None,
            output,
            result: None,
            baking: false,
            generation: 0,
            save_task: None,
            notice: None,
            notice_task: None,
            _subscriptions: subscriptions,
        };
        for step in this.session.recipe().steps {
            this.push_step(step, None, window, cx);
        }
        this.bake(None, window, cx);
        this
    }

    fn recipe(&self) -> Recipe {
        Recipe {
            steps: self.steps.iter().map(|s| s.step.clone()).collect(),
        }
    }

    fn input_dish(&self, cx: &App) -> Dish {
        match &self.input_file {
            Some(f) => Dish::Bytes(f.bytes.as_ref().clone()),
            None => Dish::Bytes(self.input.read(cx).value().as_bytes().to_vec()),
        }
    }

    fn input_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_soon(cx);
        self.recipe_changed(window, cx);
    }

    /// The recipe or its arguments changed: rebake if auto bake is on.
    fn recipe_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_soon(cx);
        if self.session.auto_bake {
            self.bake(Some(BAKE_DELAY), window, cx);
        }
        cx.notify();
    }

    /// Bake on a background thread, after `delay`, unless another request
    /// comes in first.
    fn bake(&mut self, delay: Option<Duration>, window: &mut Window, cx: &mut Context<Self>) {
        self.bake_from(0, None, delay, window, cx);
    }

    fn bake_from(
        &mut self,
        from: usize,
        dish: Option<Dish>,
        delay: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        let generation = self.generation;
        let recipe = self.recipe();
        let input = dish.unwrap_or_else(|| self.input_dish(cx));
        self.baking = true;
        cx.spawn_in(window, async move |this, cx| {
            if let Some(d) = delay {
                cx.background_executor().timer(d).await;
                let stale = this
                    .read_with(cx, |t, _| t.generation != generation)
                    .unwrap_or(true);
                if stale {
                    return;
                }
            }
            let baked = cx
                .background_executor()
                .spawn(async move { recipe.bake_from(input, from) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.show_output(baked, window, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn show_output(
        &mut self,
        baked: utilify_chef::Baked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match &baked.dish {
            Dish::Html(h) => utilify_chef::dish::html_to_text(h),
            other => other.text().into_owned(),
        };
        let chars = text.chars().count();
        let lines = if text.is_empty() {
            0
        } else {
            text.matches('\n').count() + 1
        };
        let truncated = text.len() > MAX_SHOWN;
        let shown = if truncated {
            let mut end = MAX_SHOWN;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        } else {
            text
        };
        self.output
            .update(cx, |s, cx| s.set_value(shown, window, cx));
        self.result = Some(Output {
            dish: baked.dish,
            halt: baked.halt,
            duration: baked.duration,
            chars,
            lines,
            truncated,
        });
        self.baking = false;
        cx.notify();
    }

    /// Run past the breakpoint the last bake paused at.
    fn step_past_breakpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Output {
            dish,
            halt: Some(Halt::Breakpoint { step }),
            ..
        }) = &self.result
        else {
            return;
        };
        let (dish, step) = (dish.clone(), *step);
        self.bake_from(step, Some(dish), None, window, cx);
    }

    fn toggle_auto_bake(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.auto_bake = !self.session.auto_bake;
        if self.session.auto_bake {
            self.bake(None, window, cx);
        }
        self.save_soon(cx);
        cx.notify();
    }

    fn store_session(&mut self, cx: &App) {
        self.session.recipe = self.recipe().to_json();
        let input = self.input.read(cx).value();
        self.session.set_input(if self.input_file.is_some() {
            ""
        } else {
            &input
        });
    }

    /// Write the session a second after the last change.
    fn save_soon(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok(session) = this.update(cx, |t, cx| {
                t.store_session(cx);
                t.session.clone()
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move {
                    if let Err(e) = session::save(&path, &session) {
                        log::warn!("Saving the workbench failed: {e}");
                    }
                })
                .await;
        }));
    }

    fn notify_user(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |t, cx| {
                t.notice = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let ported = utilify_chef::operations().len();
        let total: usize = utilify_chef::catalog::CATEGORIES
            .iter()
            .map(|(_, ops)| ops.len())
            .sum();
        let auto = self.session.auto_bake;
        let subtitle = ui::dotted(vec![
            div()
                .child(format!("{ported} of {total} operations"))
                .into_any_element(),
            div().child("CyberChef compatible").into_any_element(),
        ]);
        h_flex()
            .flex_none()
            .h(px(84.))
            // Clear the traffic lights.
            .pl(px(124.))
            // Line Bake up with the panels' right edge.
            .pr(px(10.))
            .gap(px(15.))
            .child(ui::icon_button(
                "workbench-icon",
                ui::icon("chef-hat", 19., fg()),
            ))
            .child(
                v_flex()
                    .flex_none()
                    .gap(px(3.))
                    .child(
                        ui::ui_text(17.5, fg())
                            .font_weight(FontWeight::MEDIUM)
                            .child("Workbench"),
                    )
                    .child(ui::ui_text(14.5, fg2()).child(subtitle)),
            )
            // Empty header space moves the window, as a title bar would.
            .child(
                div()
                    .id("drag-area")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(self.notice.clone().map(|n| {
                        ui::ui_text(14., ok())
                            .px(px(12.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .bg(ok().opacity(0.1))
                            .child(n)
                    }))
                    .on_mouse_down(MouseButton::Left, |e, window, _| {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                h_flex()
                    .id("auto-bake")
                    .gap(px(10.))
                    .cursor_pointer()
                    .child(ui::ui_text(14.5, fg2()).child("Auto bake"))
                    .child(ui::toggle("auto-bake-toggle", auto))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_auto_bake(window, cx))),
            )
            .child(
                ui::button("bake", if self.baking { "Baking…" } else { "Bake" }, true)
                    .flex_none()
                    .w(px(120.))
                    .on_click(cx.listener(|this, _, window, cx| this.bake(None, window, cx))),
            )
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(color::card())
            .font_family(UI_FONT)
            .text_color(fg())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Bake, window, cx| this.bake(None, window, cx)))
            .on_action(cx.listener(|_, _: &CloseWorkbench, window, _| window.remove_window()))
            .child(self.header(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap(px(10.))
                    .px(px(10.))
                    .pb(px(10.))
                    .child(self.operations_panel(cx).w(px(330.)).flex_none())
                    .child(self.recipe_panel(window, cx).w(px(470.)).flex_none())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(10.))
                            .child(self.input_panel(cx).flex_1())
                            .child(self.output_panel(cx).flex_1()),
                    ),
            )
    }
}

/// Where the session is kept, next to the app's settings.
fn session_path() -> Option<PathBuf> {
    utilify_core::settings::default_path().map(|p| p.with_file_name("workbench.json"))
}

/// The open workbench window, if any.
#[derive(Default)]
struct WorkbenchWindow(Option<AnyWindowHandle>);

impl Global for WorkbenchWindow {}

pub fn open(cx: &mut App) {
    if let Some(handle) = cx.try_global::<WorkbenchWindow>().and_then(|w| w.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    crate::theme::follow_system(cx);
    #[cfg(target_os = "macos")]
    cx.set_activation_policy(ActivationPolicy::Regular);
    cx.set_menus(menus());
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(gpui_kit::px(1360.), gpui_kit::px(860.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("{} Workbench", utilify_core::PRODUCT_NAME).into()),
            appears_transparent: true,
            traffic_light_position: Some(point(gpui_kit::px(16.), gpui_kit::px(22.))),
        }),
        window_min_size: Some(size(gpui_kit::px(1040.), gpui_kit::px(620.))),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        app_id: Some(utilify_core::APP_IDENTIFIER.into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workbench::new(window, cx))
    }) {
        Ok((handle, view)) => {
            cx.set_global(WorkbenchWindow(Some(handle)));
            let _ = handle.update(cx, |_, window, cx| {
                let input = view.read(cx).input.clone();
                input.update(cx, |s, cx| s.focus(window, cx));
            });
            cx.activate(true);
        }
        Err(e) => {
            log::error!("Failed to open the workbench: {e}");
            closed(cx);
        }
    }
}

/// Back to menu-bar only once the window is gone.
fn closed(cx: &mut App) {
    cx.set_global(WorkbenchWindow(None));
    #[cfg(target_os = "macos")]
    cx.set_activation_policy(ActivationPolicy::Accessory);
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: utilify_core::PRODUCT_NAME.into(),
            items: vec![
                MenuItem::action("Close Workbench", CloseWorkbench),
                MenuItem::separator(),
                MenuItem::action(format!("Quit {}", utilify_core::PRODUCT_NAME), QuitApp),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "Recipe".into(),
            items: vec![MenuItem::action("Bake", Bake)],
            disabled: false,
        },
    ]
}

/// `12,345`.
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::thousands;

    #[test]
    fn groups_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
