//! The Fans tab, after Macs Fan Control: a banner when fan control isn't
//! available (or another tool has the fans), the presets, each fan with its
//! speed and control (automatic, a constant speed, or a curve on a sensor),
//! and every temperature sensor by group.

use super::{Dashboard, text};
use crate::ui::{self, color::*, px};
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use utilify_core::fans::client::{HELPER_PLIST, INSTALL_COMMAND};
use utilify_core::fans::controller::{FanStatus, HelperStatus};
use utilify_core::fans::service::FanSnapshot;
use utilify_core::fans::{self, Active, Control, FanSettings};
use utilify_core::sensors::{self, Sensor};

/// Constant speeds move in steps of this many rpm.
const RPM_STEP: f32 = 100.;
/// Curve temperatures move in steps of this many °C, within this range.
const TEMP_STEP: f32 = 1.;
const TEMP_MIN: f32 = 20.;
const TEMP_MAX: f32 = 110.;
/// Sensor names longer than this are cut short in the pickers.
const PICKER_CHARS: usize = 24;
/// The tab scrolls past this height, so the card fits the dashboard window
/// (and the screen) with the sensor list below the fans.
const BODY_H: f32 = 880.;

/// What the tab shows, copied out of the model.
pub(super) struct Snapshot {
    fans: FanSnapshot,
    settings: FanSettings,
}

impl Snapshot {
    pub(super) fn read(stats: &crate::stats::Stats) -> Self {
        Self {
            fans: stats.fans.clone(),
            settings: stats.fan_settings.clone(),
        }
    }

    fn ready(&self) -> bool {
        self.fans.helper.is_ready()
    }
}

/// The sensors as last read, for the Settings tab's picker.
pub(super) fn sensor_list_of(s: &super::Snapshot) -> &[Sensor] {
    &s.fans.fans.sensors
}

/// Title, subtitle and chip for the tab's header.
pub(super) fn header(s: &super::Snapshot) -> (SharedString, Vec<AnyElement>, Option<SharedString>) {
    let f = &s.fans;
    let mut parts = vec![];
    match f.fans.fans.len() {
        0 if f.fans.read_at.is_some() => parts.push(text("no fans")),
        0 => parts.push(text("reading…")),
        1 => parts.push(text("1 fan")),
        n => parts.push(text(format!("{n} fans"))),
    }
    parts.push(text(f.settings.active.label().to_lowercase()));
    let chip =
        sensors::cpu_average(&f.fans.sensors).map(|c| SharedString::from(format!("{c:.0}°C")));
    ("Fans".into(), parts, chip)
}

/// `5775 rpm`.
fn rpm(v: f32) -> String {
    format!("{v:.0} rpm")
}

#[derive(Clone, Copy)]
enum Tone {
    Info,
    Caution,
    Danger,
}

/// A boxed note across the tab: an icon, a title, and lines under it.
fn banner(tone: Tone, title: &str, lines: Vec<AnyElement>) -> Div {
    let (color, icon) = match tone {
        Tone::Info => (fg2(), "info"),
        Tone::Caution => (caution(), "triangle-alert"),
        Tone::Danger => (danger(), "circle-alert"),
    };
    h_flex()
        .items_start()
        .gap(px(12.))
        .p(px(14.))
        .rounded(px(12.))
        .border_1()
        .border_color(color.opacity(0.4))
        .bg(color.opacity(0.08))
        .child(div().pt(px(2.)).child(ui::icon(icon, 17., color)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(ui::ui_text(15.5, fg()).child(title.to_string()))
                .children(lines),
        )
}

fn note(s: impl Into<SharedString>) -> AnyElement {
    ui::ui_text(14., fg2()).child(s.into()).into_any_element()
}

/// A command to copy into Terminal.
fn command(s: &str) -> AnyElement {
    ui::mono(13., fg())
        .px(px(10.))
        .py(px(6.))
        .rounded(px(8.))
        .bg(rgb(0x0c0c0c))
        .border_1()
        .border_color(card_border())
        .child(s.to_string())
        .into_any_element()
}

/// Why fan control isn't available, what else is going on, and what to do.
fn banners(s: &Snapshot) -> Vec<Div> {
    let f = &s.fans;
    let mut out = vec![];
    match &f.helper {
        HelperStatus::Ready => {}
        HelperStatus::Unknown => out.push(banner(
            Tone::Info,
            "Checking the fan helper…",
            vec![note("Speeds and temperatures are shown either way.")],
        )),
        HelperStatus::Unsupported => out.push(banner(
            Tone::Info,
            "Fan control needs an Apple Silicon Mac",
            vec![note("Fan speeds are read-only on this Mac.")],
        )),
        HelperStatus::NotInstalled => out.push(banner(
            Tone::Caution,
            "Fan helper not installed",
            vec![
                note(
                    "Setting fan speeds needs Utilify's small root helper. \
                     Run the installer from the Utilify source folder; it asks \
                     for your password once:",
                ),
                command(INSTALL_COMMAND),
                note("Until then, speeds and temperatures are read-only."),
            ],
        )),
        HelperStatus::NotRunning => out.push(banner(
            Tone::Caution,
            "Fan helper not running",
            vec![
                note("It's installed but not answering. Start it with:"),
                command(&format!("sudo launchctl bootstrap system {HELPER_PLIST}")),
                note(format!("or run {INSTALL_COMMAND} again.")),
            ],
        )),
        HelperStatus::PermissionDenied => out.push(banner(
            Tone::Caution,
            "Fan control is for admin users",
            vec![note(
                "The fan helper only takes requests from administrators of this Mac.",
            )],
        )),
        HelperStatus::Broken(e) => out.push(banner(
            Tone::Danger,
            "Lost the fan helper",
            vec![
                note(e.clone()),
                note("Retrying; the fans are back on automatic."),
            ],
        )),
    }
    if let Some(o) = &f.overheat {
        out.push(banner(
            Tone::Danger,
            "Too hot: fans on automatic",
            vec![
                note(format!("{} reached {:.0}°C.", o.sensor, o.celsius)),
                note(format!(
                    "The system is running the fans until it's below {:.0}°C, \
                     then your settings come back.",
                    fans::controller::GUARD_RELEASE_C
                )),
            ],
        ));
    }
    if f.taking_over {
        out.push(banner(
            Tone::Info,
            "Taking over the fans",
            vec![note(
                "macOS hands the fans over in a few seconds, or up to twenty \
                 right after it took them back. Your settings apply once it does.",
            )],
        ));
    }
    if f.foreign {
        out.push(banner(
            Tone::Caution,
            "Another app is controlling the fans",
            vec![note(
                "A fan tool such as Macs Fan Control holds them in manual mode. \
                 Quit it, or set it to automatic, before using fan control here.",
            )],
        ));
    }
    if let Some(e) = &f.error {
        out.push(banner(
            Tone::Danger,
            "The fan helper refused",
            vec![note(e.clone())],
        ));
    }
    out
}

fn icon_action(id: impl Into<ElementId>, icon: &str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x1f1f1f)))
        .child(ui::icon(icon, 15., fg2()))
}

fn select_preset(active: Active) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let active = active.clone();
        crate::update_fan_settings(cx, move |s, _| s.active = active);
    }
}

/// Automatic, Full blast, Custom (while it's in use) and the user's
/// presets, with save, rename and delete.
fn presets(d: &Dashboard, s: &Snapshot, cx: &mut Context<Dashboard>) -> Div {
    let ready = s.ready();
    let active = &s.settings.active;
    let row =
        |id: SharedString, title: String, detail: Option<&str>, this: Active, enabled: bool| {
            let selected = *active == this;
            h_flex()
                .id(id)
                .flex_1()
                .min_w_0()
                .h(px(40.))
                .px(px(8.))
                .gap(px(10.))
                .rounded(px(8.))
                .when(selected, |r| r.bg(raised()))
                .when(enabled && !selected, |r| {
                    r.cursor_pointer()
                        .hover(|s| s.bg(rgb(0x1a1a1a)))
                        .on_click(select_preset(this.clone()))
                })
                .when(!enabled && !selected, |r| r.opacity(0.45))
                .child(
                    div()
                        .w(px(15.))
                        .children(selected.then(|| ui::icon("check", 15., ok()))),
                )
                .child(
                    ui::ui_text(15.5, fg())
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(title),
                )
                .children(detail.map(|d| ui::ui_text(13.5, fg3()).child(d.to_string())))
        };
    let mut list = v_flex()
        .gap(px(2.))
        .child(row(
            "preset-auto".into(),
            "Automatic".into(),
            Some("the system's own control"),
            Active::Automatic,
            // Always allowed: it's how to let go.
            true,
        ))
        .child(row(
            "preset-full".into(),
            "Full blast".into(),
            Some("every fan at its maximum"),
            Active::FullBlast,
            ready,
        ));
    if *active == Active::Custom {
        list = list.child(row(
            "preset-custom".into(),
            "Custom".into(),
            Some("set below; save it to keep it"),
            Active::Custom,
            ready,
        ));
    }
    for (i, p) in s.settings.presets.iter().enumerate() {
        let name = p.name.clone();
        let rename = name.clone();
        let delete = name.clone();
        list = list.child(
            h_flex()
                .gap(px(4.))
                .child(row(
                    format!("preset-user-{i}").into(),
                    name.clone(),
                    None,
                    Active::Preset(name),
                    ready,
                ))
                .child(
                    icon_action(SharedString::from(format!("preset-rename-{i}")), "pencil")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.fan_renaming = Some(rename.clone());
                            this.fan_hint = None;
                            let value = rename.clone();
                            this.preset_name
                                .update(cx, |st, cx| st.set_value(value, window, cx));
                            cx.notify();
                        })),
                )
                .child(
                    icon_action(SharedString::from(format!("preset-delete-{i}")), "trash")
                        .on_click(move |_, _, cx| {
                            let name = delete.clone();
                            crate::update_fan_settings(cx, move |s, _| s.delete_preset(&name));
                        }),
                ),
        );
    }

    let renaming = d.fan_renaming.clone();
    let field = div()
        .flex_1()
        .min_w_0()
        .h(px(ui::SMALL_H))
        .px(px(4.))
        .flex()
        .items_center()
        .rounded(px(10.))
        .bg(control())
        .border_1()
        .border_color(control_border())
        .text_color(fg())
        .child(
            Input::new(&d.preset_name)
                .appearance(false)
                .text_size(px(14.5)),
        );
    let save_label = if renaming.is_some() { "Rename" } else { "Save" };
    let editor = h_flex()
        .gap(px(8.))
        .child(field)
        .child(
            ui::small_button("preset-save", save_label, true)
                .on_click(cx.listener(|this, _, window, cx| this.save_preset(window, cx))),
        )
        .when(renaming.is_some(), |r| {
            r.child(
                ui::small_button("preset-cancel", "Cancel", true).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.fan_renaming = None;
                        this.fan_hint = None;
                        this.preset_name
                            .update(cx, |st, cx| st.set_value("", window, cx));
                        cx.notify();
                    },
                )),
            )
        });
    let hint = d.fan_hint.clone().unwrap_or_else(|| match &renaming {
        Some(name) => format!("New name for {name}").into(),
        None => "Save the current fan settings as a preset".into(),
    });

    v_flex()
        .gap(px(8.))
        .child(ui::section_label("Presets"))
        .child(list)
        .child(div().h(px(4.)))
        .child(editor)
        .child(ui::ui_text(13.5, fg3()).child(hint))
}

impl Dashboard {
    /// Save (or rename to) the name typed in the preset field.
    pub(super) fn save_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.preset_name.read(cx).value().trim().to_string();
        let renaming = self.fan_renaming.clone();
        let stats = crate::stats::stats(cx);
        let mut ok = false;
        stats.update(cx, |s, cx| {
            s.update_fan_settings(
                |f, fans| {
                    ok = match &renaming {
                        Some(from) => f.rename_preset(from, &name),
                        None => f.save_preset(fans, &name),
                    }
                },
                cx,
            )
        });
        if ok {
            self.fan_renaming = None;
            self.fan_hint = None;
            self.preset_name
                .update(cx, |st, cx| st.set_value("", window, cx));
        } else {
            self.fan_hint = Some(if name.is_empty() {
                "Type a name first".into()
            } else {
                format!("Can't use \"{name}\": it's taken or built in").into()
            });
        }
        crate::tray::update(stats.read(cx));
        cx.notify();
    }
}

/// The next constant speed on the 100 rpm grid, inside the fan's range.
fn rpm_step(rpm: f32, min: f32, max: f32, up: bool) -> Option<f32> {
    let next = if up {
        (rpm / RPM_STEP).floor() * RPM_STEP + RPM_STEP
    } else {
        (rpm / RPM_STEP).ceil() * RPM_STEP - RPM_STEP
    };
    let next = next.clamp(min, max);
    ((next - rpm).abs() >= 1.).then_some(next)
}

fn set_control(
    index: u8,
    control: Control,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let control = control.clone();
        crate::update_fan_settings(cx, move |s, fans| s.set_control(fans, index, control));
    }
}

/// `CPU Performance Cores (av…`.
fn short(name: &str) -> String {
    if name.chars().count() <= PICKER_CHARS {
        return name.to_string();
    }
    let mut s: String = name.chars().take(PICKER_CHARS - 1).collect();
    s.push('…');
    s
}

/// The sensor before or after `id` in the list, wrapping round.
fn cycle<'a>(list: &'a [Sensor], id: &str, up: bool) -> Option<&'a Sensor> {
    if list.is_empty() {
        return None;
    }
    let n = list.len();
    Some(match list.iter().position(|s| s.id == id) {
        Some(i) if up => &list[(i + 1) % n],
        Some(i) => &list[(i + n - 1) % n],
        None => &list[0],
    })
}

/// A starting curve: CPU Core Average when there is one.
fn default_curve(sensors: &[Sensor]) -> Control {
    let id = sensors::find(sensors, sensors::CPU_AVERAGE_ID)
        .or(sensors.first())
        .map_or(sensors::CPU_AVERAGE_ID.to_string(), |s| s.id.clone());
    Control::SensorCurve {
        sensor_id: id,
        from_c: fans::DEFAULT_FROM_C,
        to_c: fans::DEFAULT_TO_C,
    }
}

/// One fan: name and speed, its range, how it's driven, and the controls
/// for that.
fn fan_row(f: &FanStatus, sensors: &[Sensor], ready: bool) -> Div {
    let i = f.info.index;
    let info = f.info;
    let mode = match f.control {
        Control::Auto => 0,
        Control::Constant(_) => 1,
        Control::SensorCurve { .. } => 2,
    };
    let start_rpm = (info.current.clamp(info.min, info.max) / RPM_STEP).round() * RPM_STEP;
    let choices = [
        Control::Auto,
        Control::Constant(start_rpm.clamp(info.min, info.max)),
        default_curve(sensors),
    ];
    let segments = ["Auto", "Constant", "Sensor"]
        .iter()
        .zip(choices)
        .enumerate()
        .map(|(k, (label, control))| {
            let seg = div()
                .id(SharedString::from(format!("fan{i}-mode{k}")))
                .child(*label);
            if ready && k != mode {
                seg.on_click(set_control(i, control))
            } else {
                seg
            }
        })
        .collect();

    let mut caption = vec![
        text(format!("min {}", rpm(info.min))),
        text(format!("max {}", rpm(info.max))),
    ];
    if let Some(t) = f.target {
        caption.push(
            div()
                .text_color(ok())
                .child(format!("target {}", rpm(t)))
                .into_any_element(),
        );
    } else if info.manual {
        caption.push(text("held by another app"));
    }

    let editor: Option<Div> = match &f.control {
        Control::Auto => None,
        Control::Constant(value) => {
            let step = |up| {
                rpm_step(*value, info.min, info.max, up)
                    .filter(|_| ready)
                    .map(|v| set_control(i, Control::Constant(v)))
            };
            Some(
                h_flex()
                    .gap(px(12.))
                    .child(ui::ui_text(14.5, fg2()).w(px(84.)).child("Speed"))
                    .child(ui::stepper(
                        format!("fan{i}-rpm"),
                        rpm(*value),
                        step(false),
                        step(true),
                    )),
            )
        }
        Control::SensorCurve {
            sensor_id,
            from_c,
            to_c,
        } => {
            let (sensor_id, from_c, to_c) = (sensor_id.clone(), *from_c, *to_c);
            let name =
                sensors::find(sensors, &sensor_id).map_or(sensor_id.clone(), |s| s.name.clone());
            let pick = |up| {
                cycle(sensors, &sensor_id, up).filter(|_| ready).map(|s| {
                    set_control(
                        i,
                        Control::SensorCurve {
                            sensor_id: s.id.clone(),
                            from_c,
                            to_c,
                        },
                    )
                })
            };
            let curve = |from: f32, to: f32| {
                set_control(
                    i,
                    Control::SensorCurve {
                        sensor_id: sensor_id.clone(),
                        from_c: from,
                        to_c: to,
                    },
                )
            };
            let ok = |v: f32| ready && (TEMP_MIN..=TEMP_MAX).contains(&v);
            let from_minus = ok(from_c - TEMP_STEP).then(|| curve(from_c - TEMP_STEP, to_c));
            let from_plus = (ok(from_c + TEMP_STEP) && from_c + TEMP_STEP < to_c)
                .then(|| curve(from_c + TEMP_STEP, to_c));
            let to_minus = (ok(to_c - TEMP_STEP) && to_c - TEMP_STEP > from_c)
                .then(|| curve(from_c, to_c - TEMP_STEP));
            let to_plus = ok(to_c + TEMP_STEP).then(|| curve(from_c, to_c + TEMP_STEP));
            let current = sensors::find(sensors, &sensor_id).map_or("not found".to_string(), |s| {
                format!("now {:.1}°C", s.celsius)
            });
            let labelled = |label: &str, stepper: Div| {
                v_flex()
                    .flex_1()
                    .gap(px(6.))
                    .child(ui::ui_text(13.5, fg3()).child(label.to_string()))
                    .child(stepper)
            };
            Some(
                v_flex()
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(ui::ui_text(14.5, fg2()).child("Sensor"))
                            .child(ui::ui_text(13.5, fg3()).child(current)),
                    )
                    .child(ui::stepper(
                        format!("fan{i}-sensor"),
                        short(&name),
                        pick(false),
                        pick(true),
                    ))
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .child(labelled(
                                "Min speed up to",
                                ui::stepper(
                                    format!("fan{i}-from"),
                                    format!("{from_c:.0}°C"),
                                    from_minus,
                                    from_plus,
                                ),
                            ))
                            .child(labelled(
                                "Max speed from",
                                ui::stepper(
                                    format!("fan{i}-to"),
                                    format!("{to_c:.0}°C"),
                                    to_minus,
                                    to_plus,
                                ),
                            )),
                    ),
            )
        }
    };

    v_flex()
        .gap(px(10.))
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(ui::ui_text(16.5, fg()).child(f.name.clone()))
                .child(ui::mono(17., fg()).child(rpm(info.current))),
        )
        .child(ui::dotted(caption).text_size(px(13.5)).text_color(fg3()))
        .child(
            div()
                .when(!ready, |d| d.opacity(0.45))
                .child(ui::segmented(segments, mode)),
        )
        .children(editor.map(|e| e.when(!ready, |d| d.opacity(0.45))))
        .children(
            f.issue
                .clone()
                .map(|m| ui::ui_text(13.5, caution()).child(m)),
        )
}

/// Every sensor, by group, in a scrolling list.
fn sensor_list(sensors: &[Sensor], read: bool) -> Div {
    let head = h_flex()
        .justify_between()
        .child(ui::section_label("Temperatures"))
        .child(ui::ui_text(13., fg4()).child("every 2 s"));
    if sensors.is_empty() {
        let msg = if read {
            "No temperature sensors on this Mac"
        } else {
            "Reading sensors…"
        };
        return v_flex()
            .gap(px(8.))
            .child(head)
            .child(ui::ui_text(14.5, fg3()).child(msg));
    }
    let mut list = v_flex().gap(px(6.));
    let mut group = None;
    for s in sensors {
        if group != Some(s.group) {
            let first = group.is_none();
            group = Some(s.group);
            list = list.child(
                ui::ui_text(13.5, fg2())
                    .pt(px(if first { 0. } else { 8. }))
                    .font_weight(FontWeight::MEDIUM)
                    .child(s.group.label()),
            );
        }
        list = list.child(ui::kv(
            s.name.clone(),
            ui::mono(14., fg()).child(format!("{:.1}°C", s.celsius)),
        ));
    }
    v_flex().gap(px(8.)).child(head).child(list)
}

/// The Fans tab.
pub(super) fn tab(d: &Dashboard, s: &super::Snapshot, cx: &mut Context<Dashboard>) -> Div {
    let f = &s.fans;
    let ready = f.ready();
    let mut fans_list = v_flex().gap(px(18.));
    if f.fans.fans.is_empty() {
        fans_list = fans_list.child(ui::ui_text(14.5, fg3()).child(if f.fans.read_at.is_some() {
            "This Mac has no fans to show"
        } else {
            "Reading fans…"
        }));
    }
    for (k, fan) in f.fans.fans.iter().enumerate() {
        if k > 0 {
            fans_list = fans_list.child(div().h(px(1.)).bg(sep()));
        }
        fans_list = fans_list.child(fan_row(fan, &f.fans.sensors, ready));
    }
    let body = v_flex()
        .gap(px(16.))
        .children(banners(f))
        .child(presets(d, f, cx))
        .child(ui::separator())
        .child(fans_list)
        .child(ui::separator())
        .child(sensor_list(&f.fans.sensors, f.fans.read_at.is_some()));
    v_flex().child(
        div()
            .id("fans-tab")
            .max_h(px(BODY_H))
            .overflow_y_scroll()
            .child(body),
    )
}

#[cfg(test)]
mod tests {
    use super::{cycle, rpm_step, short};
    use utilify_core::sensors::{Sensor, SensorGroup};

    #[test]
    fn rpm_steps_snap_to_the_grid_inside_the_range() {
        assert_eq!(rpm_step(3000., 1200., 5700., true), Some(3100.));
        assert_eq!(rpm_step(3000., 1200., 5700., false), Some(2900.));
        assert_eq!(rpm_step(3050., 1200., 5700., true), Some(3100.));
        assert_eq!(rpm_step(3050., 1200., 5700., false), Some(3000.));
        assert_eq!(rpm_step(5650., 1200., 5700., true), Some(5700.));
        assert_eq!(rpm_step(5700., 1200., 5700., true), None);
        assert_eq!(rpm_step(1250., 1200., 5700., false), Some(1200.));
        assert_eq!(rpm_step(1200., 1200., 5700., false), None);
    }

    #[test]
    fn sensor_picker_wraps_and_names_are_cut() {
        let s = |id: &str| Sensor {
            id: id.into(),
            name: id.into(),
            group: SensorGroup::Cpu,
            celsius: 40.,
        };
        let list = [s("a"), s("b"), s("c")];
        assert_eq!(cycle(&list, "c", true).unwrap().id, "a");
        assert_eq!(cycle(&list, "a", false).unwrap().id, "c");
        assert_eq!(cycle(&list, "gone", true).unwrap().id, "a");
        assert!(cycle(&[], "a", true).is_none());
        assert_eq!(short("CPU Core Average"), "CPU Core Average");
        assert_eq!(
            short("CPU Performance Cores (average)"),
            "CPU Performance Cores (…"
        );
    }
}
