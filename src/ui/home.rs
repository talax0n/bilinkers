//! Home, with its one Start button and status lines, and the waiting screen after Start.

use rune::tok::{self, type_};
use rune::{col, el, row, text, Color, El};

use crate::detect::PageKind;
use crate::session::{Connection, Sighting};
use crate::setup::CodexStatus;
use crate::ui::flow::{Codex, Flow, Input, Waiting};
use crate::ui::style::{big_button, button, card, dot, Kind, Tones, HEADING, SUBHEADING, TITLE};
use crate::ui::welcome::signed_in_text;
use crate::ui::{send, Ui};

pub const WAIT_TEXT: &str = "Log in, then open a course, unit, or activity in the browser. The bot starts by itself.";
pub const ISLAND_HINT: &str = "Open one of your courses";
pub const FINAL_TEST_REFUSED: &str = "Final tests are not run from the app";

fn header(screen: &str, setup: Option<El>, t: &tok::Theme) -> El {
    let mut header = row()
        .key("header")
        .w_full()
        .h(30.0)
        .center_y()
        .gap(8.0)
        .child(text("Bilinkers").font(HEADING).color(t.text))
        .child(text(screen.to_string()).key("screen").font(SUBHEADING).color(t.text_secondary))
        .child(el().grow(1.0));
    if let Some(setup) = setup {
        header = header.child(setup);
    }
    header
}

/// A centred card that holds a screen's content.
fn panel(key: &str, content: El, t: &tok::Theme) -> El {
    let card = card(key, t).w_full().max_w(460.0).padding(24.0).gap(16.0).child(content);
    col().w_full().grow(1.0).min_h(0.0).center().child(card)
}

fn line(key: &str, color: Color, message: String, t: &tok::Theme) -> El {
    row()
        .key(key)
        .w_full()
        .gap(10.0)
        .center_y()
        .child(dot(color))
        .child(text(message).key(format!("{key}-text")).font(type_::BODY).color(t.text).grow(1.0).min_w(0.0).truncate())
}

fn connection_line(connection: &Connection, t: &tok::Theme, tones: &Tones) -> (Color, String) {
    match connection {
        Connection::Checking => (t.text_secondary, "Checking the browser...".into()),
        Connection::NotRunning => (t.text_secondary, "Browser is closed. Start opens it.".into()),
        Connection::NoLmsTab => (tones.accent, "Browser is open".into()),
        Connection::Lms { title, .. } if !title.is_empty() => (tones.good, format!("LMS is open: {title}")),
        Connection::Lms { .. } => (tones.good, "LMS is open".into()),
    }
}

pub fn home(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let setup = button("setup", "Setup", Kind::Quiet, true, t, tones, |s: &mut Ui, tx| send(s, Input::OpenSetup, tx));
    let (codex_color, codex_text) = match &flow.codex {
        Codex::Known(CodexStatus::SignedIn(line)) => (tones.good, signed_in_text(line)),
        Codex::Known(_) => (tones.warn, "Not signed in to ChatGPT. Open Setup to sign in.".to_string()),
        Codex::Checking | Codex::SigningIn => (t.text_secondary, "Checking your sign-in...".to_string()),
    };
    let browser = &flow.browser.setup;
    let (browser_color, browser_text) = match &browser.binary {
        Ok(_) => (tones.good, format!("Browser: {}", browser.display_name())),
        Err(_) => (tones.bad, format!("{} is not installed. Open Setup to choose another.", browser.display_name())),
    };
    let (connection_color, connection_text) = connection_line(&flow.connection, t, tones);
    let start = big_button("start", "Start", flow.can_start(), t, tones, |s: &mut Ui, tx| send(s, Input::Start, tx))
        .w_full()
        .default_button();
    let content = col()
        .w_full()
        .gap(16.0)
        .child(
            col().gap(6.0).child(text("Ready when you are").key("home-title").font(TITLE).color(t.text)).child(
                text("Press Start. The bot opens your browser and begins once you open a course, unit, or activity.")
                    .font(type_::BODY)
                    .color(t.text_secondary)
                    .wrap(),
            ),
        )
        .child(start)
        .child(el().w_full().h(1.0).fill(t.separator))
        .child(
            col()
                .key("status")
                .role("status")
                .w_full()
                .gap(8.0)
                .child(line("codex-line", codex_color, codex_text, t))
                .child(line("browser-line", browser_color, browser_text, t))
                .child(line("connection-line", connection_color, connection_text, t)),
        );
    col().key("home").w_full().h_full().gap(12.0).child(header("Home", Some(setup), t)).child(panel(
        "home-card",
        content,
        t,
    ))
}

/// The waiting screen's status: its colour, its text, and a hint or detail below it.
fn waiting_status(waiting: &Waiting, t: &tok::Theme, tones: &Tones) -> (Color, String, Option<(Color, String)>) {
    let sighting = match waiting {
        Waiting::Opening => return (tones.accent, "Opening your browser...".into(), None),
        Waiting::OpenFailed(e) => {
            return (tones.bad, "The browser did not open.".into(), Some((t.text_secondary, e.clone())))
        }
        Waiting::Looking(None) => return (tones.accent, "Looking at the browser...".into(), None),
        Waiting::Looking(Some(sighting)) => sighting,
    };
    match sighting {
        Sighting::NotRunning => (tones.accent, "Waiting for the browser to open...".into(), None),
        Sighting::NoLmsTab => (tones.accent, "Waiting for the LMS to open...".into(), None),
        Sighting::Lms { title, page, .. } => {
            let on = if title.is_empty() { "Waiting for a course page...".to_string() } else { format!("On: {title}") };
            match page {
                Some(PageKind::IslandMap) => (tones.accent, on, Some((tones.good, ISLAND_HINT.into()))),
                Some(PageKind::FinalTest) => (
                    tones.warn,
                    FINAL_TEST_REFUSED.into(),
                    Some((t.text_secondary, "Open another page to continue.".into())),
                ),
                _ => (tones.accent, on, None),
            }
        }
    }
}

pub fn waiting(flow: &Flow, waiting: &Waiting, t: &tok::Theme, tones: &Tones) -> El {
    let (color, status, hint) = waiting_status(waiting, t, tones);
    let mut status_block = col().key("wait-status").role("status").w_full().gap(6.0).child(
        row().w_full().gap(10.0).center_y().child(dot(color)).child(
            text(status).key("wait-status-text").font(type_::TITLE).color(t.text).grow(1.0).min_w(0.0).truncate(),
        ),
    );
    if let Some((color, hint)) = hint {
        status_block = status_block.child(text(hint).key("wait-hint").font(type_::BODY).color(color).pl(18.0).wrap());
    }
    let cancel =
        button("cancel", "Cancel", Kind::Secondary, true, t, tones, |s: &mut Ui, tx| send(s, Input::Cancel, tx));
    let mut actions = row().key("actions").w_full().gap(10.0).center_y().child(el().grow(1.0)).child(cancel);
    if matches!(waiting, Waiting::OpenFailed(_)) {
        let retry = button("start", "Try again", Kind::Primary, flow.can_start(), t, tones, |s: &mut Ui, tx| {
            send(s, Input::Start, tx)
        })
        .default_button();
        actions = actions.child(retry);
    }
    let content = col()
        .w_full()
        .gap(16.0)
        .child(
            col()
                .gap(6.0)
                .child(text("Open your course").key("wait-title").font(TITLE).color(t.text))
                .child(text(WAIT_TEXT).key("wait-text").font(type_::BODY).color(t.text_secondary).wrap()),
        )
        .child(el().w_full().h(1.0).fill(t.separator))
        .child(status_block)
        .child(actions);
    col().key("waiting").w_full().h_full().gap(12.0).child(header("Waiting", None, t)).child(panel(
        "wait-card",
        content,
        t,
    ))
}
