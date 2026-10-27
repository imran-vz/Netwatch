//! Input and output. Input is typed text, or a file's raw bytes (opened or
//! dropped). Output shows the last bake as text, with its size and time; it
//! copies, saves to a file, or becomes the next input.

use super::widgets::{chip, panel, text_button, tool_button};
use super::{InputFile, MAX_SHOWN, Workbench, thousands};
use crate::ui::{self, color::*, px};
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use utilify_chef::Dish;
use utilify_chef::recipe::Halt;
use utilify_core::format;

impl Workbench {
    /// Use `bytes` as the input in place of the text field.
    pub(super) fn set_input_bytes(
        &mut self,
        name: SharedString,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_file = Some(InputFile {
            name,
            bytes: Arc::new(bytes),
        });
        self.input_changed(window, cx);
    }

    fn clear_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_file = None;
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.input_changed(window, cx);
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use as Input".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                Self::load_file(this, path, cx).await;
            }
        })
        .detach();
    }

    fn drop_files(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = paths.paths().first().cloned() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            Self::load_file(this, path, cx).await
        })
        .detach();
    }

    async fn load_file(this: WeakEntity<Self>, path: PathBuf, cx: &mut AsyncWindowContext) {
        let read = path.clone();
        let bytes = cx
            .background_executor()
            .spawn(async move { std::fs::read(read) })
            .await;
        let _ = this.update_in(cx, |this, window, cx| match bytes {
            Ok(bytes) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                this.set_input_bytes(name.into(), bytes, window, cx);
            }
            Err(e) => this.notify_user(format!("Couldn't open {}: {e}", path.display()), cx),
        });
    }

    /// Text output as UTF-8; byte output exactly as produced.
    fn output_bytes(&self) -> Option<Vec<u8>> {
        let out = self.result.as_ref()?;
        Some(match &out.dish {
            Dish::Bytes(b) => b.clone(),
            Dish::Text(t) => t.clone().into_bytes(),
            Dish::Html(h) => utilify_chef::dish::html_to_text(h).into_bytes(),
        })
    }

    fn copy_output(&mut self, cx: &mut Context<Self>) {
        let Some(out) = &self.result else { return };
        let text = match &out.dish {
            Dish::Html(h) => utilify_chef::dish::html_to_text(h),
            other => other.text().into_owned(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notify_user("Output copied", cx);
    }

    fn save_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bytes) = self.output_bytes() else {
            return;
        };
        let dir = dirs_downloads();
        let path = cx.prompt_for_new_path(&dir, Some("output.dat"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move { std::fs::write(target, bytes) })
                .await;
            let _ = this.update(cx, |this, cx| match written {
                Ok(()) => this.notify_user(format!("Saved {}", path.display()), cx),
                Err(e) => this.notify_user(format!("Couldn't save: {e}"), cx),
            });
        })
        .detach();
    }

    /// Make the output the next input, as CyberChef's "replace input with
    /// output" does. Binary output stays bytes.
    fn output_to_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(out) = &self.result else { return };
        match &out.dish {
            Dish::Bytes(b) if std::str::from_utf8(b).is_err() => {
                let bytes = b.clone();
                self.set_input_bytes("Previous output".into(), bytes, window, cx);
            }
            dish => {
                let text = match dish {
                    Dish::Html(h) => utilify_chef::dish::html_to_text(h),
                    other => other.text().into_owned(),
                };
                self.input_file = None;
                self.input.update(cx, |s, cx| s.set_value(text, window, cx));
                self.input_changed(window, cx);
            }
        }
    }

    pub(super) fn input_panel(&self, cx: &mut Context<Self>) -> Div {
        let caption = match &self.input_file {
            Some(f) => ui::dotted(vec![
                div().child(f.name.clone()).into_any_element(),
                div()
                    .child(format::bytes(f.bytes.len() as u64))
                    .into_any_element(),
            ]),
            None => {
                let text = self.input.read(cx).value();
                let lines = if text.is_empty() {
                    0
                } else {
                    text.matches('\n').count() + 1
                };
                ui::dotted(vec![
                    div()
                        .child(format!("length {}", thousands(text.chars().count())))
                        .into_any_element(),
                    div()
                        .child(format!("lines {}", thousands(lines)))
                        .into_any_element(),
                ])
            }
        };
        let actions = h_flex()
            .gap(px(6.))
            .child(
                text_button("open-file", "Open file")
                    .on_click(cx.listener(|this, _, window, cx| this.open_file(window, cx))),
            )
            .child(
                tool_button("clear-input", "trash", fg2())
                    .on_click(cx.listener(|this, _, window, cx| this.clear_input(window, cx))),
            );
        let body = match &self.input_file {
            Some(f) => file_card(f, cx).into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .p(px(10.))
                .font_family(crate::theme::MONO_FONT)
                .text_size(px(15.))
                .child(Textarea::new(&self.input).appearance(false).h_full())
                .into_any_element(),
        };
        panel(
            "Input",
            caption,
            actions,
            v_flex().flex_1().min_h_0().child(body),
        )
        .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(focus()))
        .on_drop(
            cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_files(paths, window, cx)
            }),
        )
    }

    pub(super) fn output_panel(&self, cx: &mut Context<Self>) -> Div {
        let out = self.result.as_ref();
        let mut parts: Vec<AnyElement> = vec![];
        if let Some(o) = out {
            parts.push(
                div()
                    .child(format!("length {}", thousands(o.chars)))
                    .into_any_element(),
            );
            parts.push(
                div()
                    .child(format!("lines {}", thousands(o.lines)))
                    .into_any_element(),
            );
            parts.push(div().child(duration(o.duration)).into_any_element());
        }
        let caption = h_flex()
            .gap(px(8.))
            .child(ui::dotted(parts))
            .when(self.baking, |c| c.child(chip("baking", fg3())))
            .when(out.is_some_and(|o| o.dish.is_binary()), |c| {
                c.child(chip("binary", caution()))
            });
        let has_output = out.is_some_and(|o| !o.dish.is_empty());
        let paused = matches!(
            out.and_then(|o| o.halt.as_ref()),
            Some(Halt::Breakpoint { .. })
        );
        let actions = h_flex()
            .gap(px(6.))
            .when(paused, |a| {
                a.child(text_button("step", "Step").text_color(caution()).on_click(
                    cx.listener(|this, _, window, cx| this.step_past_breakpoint(window, cx)),
                ))
            })
            .when(has_output, |a| {
                a.child(
                    text_button("to-input", "Use as input").on_click(
                        cx.listener(|this, _, window, cx| this.output_to_input(window, cx)),
                    ),
                )
                .child(
                    text_button("save-output", "Save")
                        .on_click(cx.listener(|this, _, window, cx| this.save_output(window, cx))),
                )
                .child(
                    text_button("copy-output", "Copy")
                        .on_click(cx.listener(|this, _, _, cx| this.copy_output(cx))),
                )
            });

        let banner = out.and_then(|o| {
            let (text, color): (String, Hsla) = match &o.halt {
                Some(Halt::Error { step, .. }) => {
                    let name = self.steps.get(*step).map_or("A step", |s| s.step.op.name);
                    (format!("{name} failed; its message is below"), danger())
                }
                Some(Halt::Breakpoint { step }) => {
                    let name = self.steps.get(*step).map_or("a step", |s| s.step.op.name);
                    (
                        format!("Paused before {name}. Step runs on from here."),
                        caution(),
                    )
                }
                None if o.truncated => (
                    format!(
                        "Showing the first {}; Copy and Save take all of it",
                        format::bytes(MAX_SHOWN as u64)
                    ),
                    fg3(),
                ),
                None => return None,
            };
            Some(
                ui::ui_text(13.5, color)
                    .flex_none()
                    .mx(px(10.))
                    .mt(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(9.))
                    .bg(color.opacity(0.1))
                    .child(text),
            )
        });
        let failed = matches!(out.and_then(|o| o.halt.as_ref()), Some(Halt::Error { .. }));
        panel(
            "Output",
            caption,
            actions,
            v_flex().flex_1().min_h_0().children(banner).child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(10.))
                    .font_family(crate::theme::MONO_FONT)
                    .text_size(px(15.))
                    .when(failed, |d| d.text_color(danger()))
                    .child(
                        Textarea::new(&self.output)
                            .appearance(false)
                            .readonly(true)
                            .h_full(),
                    ),
            ),
        )
    }
}

fn file_card(f: &InputFile, cx: &mut Context<Workbench>) -> Div {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(ui::icon("file", 34., fg3()))
        .child(ui::ui_text(15.5, fg()).child(f.name.clone()))
        .child(ui::mono(13.5, fg3()).child(format!(
            "{} · {} bytes",
            format::bytes(f.bytes.len() as u64),
            thousands(f.bytes.len())
        )))
        .child(
            text_button("type-input", "Type input instead")
                .on_click(cx.listener(|this, _, window, cx| this.clear_input(window, cx))),
        )
}

/// `4 ms`, `1.2 s`.
fn duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "<1 ms".into()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

fn dirs_downloads() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}
