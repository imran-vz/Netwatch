//! Keep awake in the dashboard: a compact control on Overview, and the Awake
//! tab with everything Amphetamine's menu offers: the current session and its
//! options, every way to start one (including a custom length and an end
//! time), and the quick settings.

use super::{state, text};
use crate::ui::{self, color::*, px};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use netwatch_core::awake::{self, Activation, Mode, Session};
use netwatch_core::format;
use netwatch_core::settings::Settings;
use std::time::{Duration, Instant};

/// What the keep-awake views show, copied out of the model.
pub(super) struct Snapshot {
    session: Option<Session>,
    status: Option<String>,
    total: Duration,
}

impl Snapshot {
    pub(super) fn read(stats: &crate::stats::Stats) -> Self {
        let k = &stats.keep_awake;
        Self {
            session: k.session().cloned(),
            status: k.status(Instant::now()),
            total: k.total(),
        }
    }

    fn mode(&self) -> Option<Mode> {
        self.session.as_ref().map(|s| s.mode)
    }
}

/// The Awake tab's custom length and end time, kept while the app runs.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Picks {
    for_mins: Option<u32>,
    until: Option<u16>,
}

const FOR_DEFAULT: u32 = 90;
const FOR_MAX: u32 = 24 * 60;
const UNTIL_STEP: u16 = 15;

impl Picks {
    fn for_mins(&self) -> u32 {
        self.for_mins.unwrap_or(FOR_DEFAULT)
    }

    /// Defaults to the first whole hour at least 30 minutes away.
    fn until(&self) -> u16 {
        self.until.unwrap_or_else(|| {
            let now = awake::local_secs_of_day() / 60;
            ((now + 30).div_ceil(60) * 60 % 1440) as u16
        })
    }
}

/// 5-minute steps up to an hour, then 15.
fn for_step(mins: u32, up: bool) -> Option<u32> {
    let next = match (up, mins) {
        (true, m) if m < 60 => m + 5,
        (true, m) => m + 15,
        (false, m) if m <= 60 => m.checked_sub(5)?,
        (false, m) => m - 15,
    };
    (5..=FOR_MAX).contains(&next).then_some(next)
}

/// Quarter-hour steps around the clock.
fn until_step(minute: u16, up: bool) -> u16 {
    let m = minute - minute % UNTIL_STEP;
    if up {
        (m + UNTIL_STEP) % 1440
    } else if m == minute {
        (m + 1440 - UNTIL_STEP) % 1440
    } else {
        m
    }
}

/// Choices on the Overview's keep-awake control.
const OVERVIEW_CHOICES: [Option<Mode>; 6] = [
    None,
    Some(Mode::Indefinite),
    Some(Mode::For(Duration::from_secs(30 * 60))),
    Some(Mode::For(Duration::from_secs(60 * 60))),
    Some(Mode::For(Duration::from_secs(2 * 60 * 60))),
    Some(Mode::WhileFileDownloading),
];

fn start(mode: Mode) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| crate::start_keep_awake(cx, mode, Activation::Manual)
}

fn status_text(s: &Snapshot) -> AnyElement {
    match &s.status {
        Some(status) => ui::mono(15., ok()).child(status.clone()).into_any_element(),
        None => ui::ui_text(14.5, fg3()).child("off").into_any_element(),
    }
}

fn kept_awake(s: &Snapshot) -> String {
    format!("kept awake {}", format::span_long(s.total.as_secs()))
}

/// Status, one-click choices, and the time kept awake so far.
pub(super) fn overview_section(s: &super::Snapshot) -> Div {
    let a = &s.awake;
    let segments = OVERVIEW_CHOICES
        .iter()
        .enumerate()
        .map(|(i, &mode)| {
            let seg = div().id(("awake", i));
            let seg = match mode {
                None => seg.on_click(|_, _, cx| crate::stop_keep_awake(cx)),
                Some(m) => seg.on_click(start(m)),
            };
            match mode {
                None => seg.child("Off"),
                Some(Mode::Indefinite) => seg.child("∞"),
                Some(Mode::For(d)) => seg.child(format::span(d.as_secs())),
                Some(_) => seg.child(ui::icon("file-down", 15., fg2())),
            }
        })
        .collect();
    // A session started another way selects nothing.
    let on = OVERVIEW_CHOICES
        .iter()
        .position(|&m| m == a.mode())
        .unwrap_or(usize::MAX);
    let display = if s.settings.awake_allow_display_sleep {
        "display may sleep"
    } else {
        "display stays on"
    };
    v_flex()
        .gap(px(12.))
        .child(ui::label_row("Keep awake", status_text(a)))
        .child(ui::segmented(segments, on))
        .child(ui::caption(
            div().text_color(fg3()).child(kept_awake(a)),
            div().text_color(fg3()).child(display),
        ))
}

/// Title and subtitle for the tab's header.
pub(super) fn header(s: &super::Snapshot) -> (SharedString, Vec<AnyElement>) {
    let a = &s.awake;
    let mut parts = vec![];
    match &a.session {
        Some(session) => {
            parts.push(
                div()
                    .text_color(ok())
                    .child(a.status.clone().unwrap_or_default())
                    .into_any_element(),
            );
            parts.push(text(match session.activation {
                Activation::Manual => "manual",
                Activation::AtLaunch => "at launch",
            }));
        }
        None => parts.push(text("not active")),
    }
    ("Keep Awake".into(), parts)
}

fn section_label(label: &str) -> Div {
    ui::ui_text(13., fg3()).child(label.to_uppercase())
}

/// Title (and detail) on the left, a control on the right.
fn row(title: &str, detail: Option<String>, control: impl IntoElement) -> Div {
    h_flex()
        .gap(px(12.))
        .min_h(px(40.))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(ui::ui_text(15.5, fg()).child(title.to_string()))
                .children(detail.map(|d| ui::ui_text(13.5, fg3()).child(d))),
        )
        .child(control)
}

fn small_button(id: impl Into<ElementId>, label: &str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(33.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .border_1()
        .border_color(card_border())
        .bg(rgb(0x141414))
        .text_size(px(14.5))
        .text_color(fg())
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x1f1f1f)))
        .child(label.to_string())
}

fn toggle_row(
    id: &'static str,
    title: &str,
    detail: Option<String>,
    on: bool,
    enabled: bool,
    flip: fn(&mut Settings),
) -> Div {
    let toggle = ui::toggle(id, on).when(enabled, |t| {
        t.on_click(move |_, _, cx| crate::update_settings(cx, flip))
    });
    row(title, detail, toggle).when(!enabled, |r| r.opacity(0.45))
}

/// The Awake tab.
pub(super) fn tab(s: &super::Snapshot, cx: &mut App) -> Div {
    let a = &s.awake;
    let picks = state(cx).awake;
    let set = s.settings.clone();

    let session = {
        let mut v = v_flex()
            .gap(px(10.))
            .child(ui::label_row("Current session", status_text(a)));
        if let Some(session) = &a.session {
            let running = format::span_long(session.started.elapsed().as_secs());
            v = v.child(row(
                session.activation.label(),
                Some(format!("running {running}")),
                small_button("awake-end", "End session")
                    .text_color(danger())
                    .on_click(|_, _, cx| crate::stop_keep_awake(cx)),
            ));
        }
        v
    };

    let for_mins = picks.for_mins();
    let until = picks.until();
    let stepper = |id: &'static str, value: String, minus, plus| {
        div()
            .w(px(175.))
            .flex()
            .child(ui::stepper(id, value, minus, plus))
    };
    let pick = |f: fn(&mut Picks, Option<u32>, Option<u16>), m: Option<u32>, u: Option<u16>| {
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            f(&mut state(cx).awake, m, u);
            window.refresh();
        }
    };
    let set_for: fn(&mut Picks, Option<u32>, Option<u16>) = |p, m, _| p.for_mins = m;
    let set_until: fn(&mut Picks, Option<u32>, Option<u16>) = |p, _, u| p.until = u;
    let starts = v_flex()
        .gap(px(6.))
        // Apps change too often to list here; the menu lists the running ones.
        .child(
            h_flex()
                .justify_between()
                .child(section_label("Start new session"))
                .child(ui::ui_text(13., fg4()).child("while an app runs: in the menu")),
        )
        .child(row(
            "Indefinitely",
            None,
            small_button("awake-indefinite", "Start").on_click(start(Mode::Indefinite)),
        ))
        .child(row(
            "For",
            None,
            h_flex()
                .gap(px(10.))
                .child(stepper(
                    "awake-for",
                    format::span_long(for_mins as u64 * 60),
                    for_step(for_mins, false).map(|m| pick(set_for, Some(m), None)),
                    for_step(for_mins, true).map(|m| pick(set_for, Some(m), None)),
                ))
                .child(
                    small_button("awake-for-start", "Start")
                        .on_click(start(Mode::For(Duration::from_secs(for_mins as u64 * 60)))),
                ),
        ))
        .child(row(
            "Until",
            None,
            h_flex()
                .gap(px(10.))
                .child(stepper(
                    "awake-until",
                    awake::clock(until),
                    Some(pick(set_until, None, Some(until_step(until, false)))),
                    Some(pick(set_until, None, Some(until_step(until, true)))),
                ))
                .child(
                    small_button("awake-until-start", "Start").on_click(start(Mode::Until(until))),
                ),
        ))
        .child(row(
            "While a file downloads",
            Some("Any download in progress in Downloads".into()),
            small_button("awake-file", "Start").on_click(start(Mode::WhileFileDownloading)),
        ))
        .child(row(
            "While the network is busy",
            Some(format!(
                "Ends after {} under {}",
                format::span(awake::NETWORK_GRACE.as_secs()),
                format::rate(awake::NETWORK_BUSY)
            )),
            small_button("awake-network", "Start").on_click(start(Mode::WhileNetworkBusy)),
        ));

    let screen_saver_mins = set.awake_screen_saver_mins;
    let set_mins = |m: u16| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            crate::update_settings(cx, |s| s.awake_screen_saver_mins = m)
        }
    };
    let options = v_flex()
        .gap(px(6.))
        .child(section_label("Session options"))
        .child(toggle_row(
            "awake-allow-display",
            "Allow display sleep",
            None,
            set.awake_allow_display_sleep,
            true,
            |s| s.awake_allow_display_sleep = !s.awake_allow_display_sleep,
        ))
        .child(toggle_row(
            "awake-allow-closed",
            "Allow sleep with the lid closed",
            Some("Blocking it only works on AC power".into()),
            set.awake_allow_closed_display_sleep,
            true,
            |s| s.awake_allow_closed_display_sleep = !s.awake_allow_closed_display_sleep,
        ))
        .child(toggle_row(
            "awake-screen-saver",
            "Allow the screen saver",
            // With the stepper showing below, the delay needn't repeat here.
            (!set.awake_screen_saver).then(|| {
                format!(
                    "After {} without input",
                    format::span_long(set.awake_screen_saver_after().as_secs())
                )
            }),
            set.awake_screen_saver,
            !set.awake_allow_display_sleep,
            |s| s.awake_screen_saver = !s.awake_screen_saver,
        ))
        .when(
            set.awake_screen_saver && !set.awake_allow_display_sleep,
            |v| {
                v.child(row(
                    "Screen saver after",
                    None,
                    div().w(px(175.)).flex().child(ui::stepper(
                        "awake-saver-mins",
                        format::span_long(screen_saver_mins as u64 * 60),
                        (screen_saver_mins > 5).then(|| set_mins(screen_saver_mins - 5)),
                        (screen_saver_mins < 240).then(|| set_mins(screen_saver_mins + 5)),
                    )),
                ))
            },
        );

    let login = crate::system::login::enabled();
    let quick = v_flex()
        .gap(px(6.))
        .child(section_label("Quick settings"))
        .child(toggle_row(
            "awake-on-launch",
            "Start a session at launch",
            Some("Indefinitely, when Netwatch opens".into()),
            set.awake_on_launch,
            true,
            |s| s.awake_on_launch = !s.awake_on_launch,
        ))
        .child(toggle_row(
            "awake-notify-end",
            "Notify when a session ends",
            None,
            set.awake_notify_end,
            true,
            |s| s.awake_notify_end = !s.awake_notify_end,
        ))
        .child({
            let on = login == Some(true);
            let toggle = ui::toggle("launch-at-login", on).when(login.is_some(), |t| {
                t.on_click(|_, _, cx| crate::toggle_launch_at_login(cx))
            });
            row(
                "Launch Netwatch at login",
                login.is_none().then(|| "Needs macOS 13 or later".into()),
                toggle,
            )
            .when(login.is_none(), |r| r.opacity(0.45))
        });

    v_flex()
        .gap(px(16.))
        .child(session)
        .child(ui::separator())
        .child(starts)
        .child(ui::separator())
        .child(options)
        .child(ui::separator())
        .child(quick)
        .child(ui::caption(
            div().text_color(fg3()).child(kept_awake(a)),
            div(),
        ))
}

#[cfg(test)]
mod tests {
    use super::{for_step, until_step};

    #[test]
    fn custom_length_steps() {
        assert_eq!(for_step(5, false), None);
        assert_eq!(for_step(5, true), Some(10));
        assert_eq!(for_step(55, true), Some(60));
        assert_eq!(for_step(60, true), Some(75));
        assert_eq!(for_step(60, false), Some(55));
        assert_eq!(for_step(75, false), Some(60));
        assert_eq!(for_step(24 * 60, true), None);
    }

    #[test]
    fn end_time_steps_wrap_round_the_clock() {
        assert_eq!(until_step(23 * 60 + 45, true), 0);
        assert_eq!(until_step(0, false), 23 * 60 + 45);
        // Off the grid: snap to it first.
        assert_eq!(until_step(9 * 60 + 7, true), 9 * 60 + 15);
        assert_eq!(until_step(9 * 60 + 7, false), 9 * 60);
    }
}
