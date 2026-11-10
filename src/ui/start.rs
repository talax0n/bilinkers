//! The start screen: the mode picker, the browser from `.env`, the debug port's state, and the
//! Open browser and Start buttons.

use rune::tok::{self, type_};
use rune::{col, el, row, text, Cursor, El, TextStyle};

use crate::session::{Connection, RunMode};
use crate::ui::flow::{Flow, Input, Opening};
use crate::ui::style::{button, card, dot, section, Kind, Tones, HEADING, SUBHEADING};
use crate::ui::{send, Ui};

const CHOICE: TextStyle = TextStyle::new(13.0, 500.0);

/// The picker's rows: the mode, its key and what it does, Auto-detect first.
const CHOICES: [(Option<RunMode>, &str, &str); 8] = [
    (None, "auto", "Reads the open page"),
    (Some(RunMode::Course), "course", "Unfinished Units and Checkpoints"),
    (Some(RunMode::Unit), "unit", "Unchecked activities in a unit"),
    (Some(RunMode::Exercise), "exercise", "Options until Check says correct"),
    (Some(RunMode::Iframe), "iframe", "An LTI activity in an iframe"),
    (Some(RunMode::Reading), "reading", "A grouped reading passage"),
    (Some(RunMode::Checkpoint), "checkpoint", "Attempts until the score is 100"),
    (Some(RunMode::FinalTest), "final-test", "Writes an answer key"),
];

pub const FINAL_TEST_NOTE: &str = "Read-only: writes an answer key, never clicks Submit";
pub const AUTO_ANSWER_NOTE: &str = "FINAL_TEST_AUTO_ANSWER=1 is set, so confident answers are also clicked and saved.";

fn radio(choice: Option<RunMode>, key: &str, hint: &str, selected: bool, t: &tok::Theme, tones: &Tones) -> El {
    let label = choice.map_or("Auto-detect", RunMode::label);
    let mark = if selected {
        el().size(16.0, 16.0).radius(8.0).border(5.0, tones.accent).fill(tones.on_accent)
    } else {
        el().size(16.0, 16.0).radius(8.0).border(1.0, t.control_off).fill(t.card)
    };
    row()
        .key(format!("mode-{key}"))
        .role("radio")
        .label(label)
        .checked(selected)
        .focusable(true)
        .focus_selected(selected)
        .focus_ring(2.0, 0.0, tones.accent)
        .w_full()
        .h(30.0)
        .px(8.0)
        .gap(10.0)
        .center_y()
        .radius(6.0)
        .fill(if selected { t.press } else { t.card })
        .hover(t.hover)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |s: &mut Ui, tx| send(s, Input::Pick(choice), tx))
        .child(mark)
        .child(text(label).font(CHOICE).color(t.text).w(92.0))
        .child(text(hint).font(type_::BODY).color(t.text_secondary).grow(1.0).min_w(0.0).truncate())
}

fn picker(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let mut group = col().key("modes").role("radiogroup").label("Mode").focus_group().focus_follows().gap(2.0);
    for (choice, key, hint) in CHOICES {
        group = group.child(radio(choice, key, hint, flow.choice == choice, t, tones));
    }
    let mut picker = card("mode-card", t).gap(8.0).child(section("Mode", t)).child(group);
    if flow.choice == Some(RunMode::FinalTest) {
        let mut note = col()
            .key("final-test-note")
            .gap(4.0)
            .pt(4.0)
            .child(text(FINAL_TEST_NOTE).key("final-test-read-only").font(type_::LABEL).color(tones.warn).wrap());
        if flow.auto_answer {
            note = note
                .child(text(AUTO_ANSWER_NOTE).key("final-test-auto").font(type_::BODY).color(t.text_secondary).wrap());
        }
        picker = picker.child(note);
    }
    picker
}

fn browser_name(name: &str) -> String {
    match name {
        "chrome" => "Chrome".into(),
        "brave" => "Brave".into(),
        "edge" => "Edge".into(),
        other => other.into(),
    }
}

fn browser(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let setup = &flow.browser;
    let mut info = col().gap(2.0).w_full().child(
        row()
            .gap(8.0)
            .center_y()
            .child(text(browser_name(&setup.name)).key("browser-name").font(type_::TITLE).color(t.text))
            .child(text(format!("BROWSER={}", setup.name)).font(type_::BODY).color(t.text_secondary)),
    );
    info = match &setup.binary {
        Ok(path) => {
            info.child(text(path.clone()).key("browser-path").font(type_::BODY).color(t.text_secondary).wrap_anywhere())
        }
        Err(e) => info.child(text(e.clone()).key("browser-error").font(type_::BODY).color(tones.bad).wrap()),
    };
    let label = if flow.opening == Opening::Busy { "Opening" } else { "Open browser" };
    let open = button("open-browser", label, Kind::Secondary, flow.can_open_browser(), t, tones, |s: &mut Ui, tx| {
        send(s, Input::OpenBrowser, tx)
    });
    let actions = row()
        .gap(10.0)
        .center_y()
        .w_full()
        .child(open)
        .child(text("Change BROWSER in .env").key("browser-hint").font(type_::BODY).color(t.text_secondary));
    let mut block = card("browser", t).gap(8.0).child(section("Browser", t)).child(info).child(actions);
    if let Opening::Failed(e) = &flow.opening {
        block = block.child(text(e.clone()).key("open-error").font(type_::BODY).color(tones.bad).wrap());
    }
    block
}

/// The status line, its colour and what to do next.
fn connection_text(connection: &Connection, t: &tok::Theme, tones: &Tones) -> (String, rune::Color, &'static str) {
    match connection {
        Connection::Checking => ("Checking the browser".into(), t.text_secondary, "Looking at the debug port."),
        Connection::NotRunning => {
            ("Browser not running".into(), t.text_secondary, "Open the browser, then log in to the LMS.")
        }
        Connection::NoLmsTab => {
            ("Browser running, no LMS tab".into(), tones.warn, "Log in and open the exercise in the browser.")
        }
        Connection::Lms { title, url } => {
            let name = if title.is_empty() { url } else { title };
            (format!("LMS tab: {name}"), tones.good, "Open the page to work on, then press Start.")
        }
    }
}

fn connection(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let (status, color, hint) = connection_text(&flow.connection, t, tones);
    let mut block = card("connection", t).gap(6.0).child(section("Connection", t)).child(
        row().gap(8.0).center_y().w_full().child(dot(color)).child(
            text(status).key("connection-status").font(type_::TITLE).color(t.text).grow(1.0).min_w(0.0).truncate(),
        ),
    );
    if let Connection::Lms { url, title } = &flow.connection {
        if !title.is_empty() {
            block = block
                .child(text(url.clone()).key("connection-url").font(type_::BODY).color(t.text_secondary).truncate());
        }
    }
    block.child(text(hint).key("connection-hint").font(type_::BODY).color(t.text_secondary).wrap())
}

pub fn view(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let header = row()
        .key("header")
        .w_full()
        .center_y()
        .gap(8.0)
        .child(text("Beelingua Bot").font(HEADING).color(t.text))
        .child(text("new run").key("mode").font(SUBHEADING).color(t.text_secondary));
    let start =
        button("start", "Start", Kind::Primary, flow.can_start(), t, tones, |s: &mut Ui, tx| send(s, Input::Start, tx))
            .default_button();
    let footer = row().key("footer").w_full().center_y().gap(10.0).child(el().grow(1.0)).child(start);
    let side = col()
        .key("side")
        .grow(1.0)
        .min_w(0.0)
        .gap(12.0)
        .child(browser(flow, t, tones))
        .child(connection(flow, t, tones))
        .child(el().grow(1.0))
        .child(footer);
    let body = row()
        .key("body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(12.0)
        .child(picker(flow, t, tones).w(380.0).shrink().self_start())
        .child(side);
    col().key("start-screen").w_full().h_full().gap(12.0).child(header).child(body)
}
