//! Home, with its one Start button and status rows, and the waiting screen after Start.

use rune::{col, el, row, text, Color, El};

use crate::detect::PageKind;
use crate::session::{Connection, Sighting};
use crate::setup::CodexStatus;
use crate::ui::flow::{Codex, Flow, Input, Waiting};
use crate::ui::style::{
    big_button, button, card, chip, dot, font, glyph, hairline, list_row, live_dot, marker, notice, page, pill,
    section, space, Header, Kind, Mark, Palette, Tone, CHECK,
};
use crate::ui::{send, Ui};

pub const WAIT_TEXT: &str =
    "Log in, then open the course map, a course, a unit, or an activity in the browser. The bot starts by itself.";
pub const FINAL_TEST_REFUSED: &str = "Final tests are not run from the app";

/// A status row: a dot, what it is about, and its state on the right.
fn status_row(key: &str, color: Color, label: &str, value: String, p: &Palette) -> El {
    list_row(key, false, p)
        .child(dot(color))
        .child(text(label.to_string()).font(font::STRONG).color(p.ink).w(110.0))
        .child(
            text(value)
                .key(format!("{key}-text"))
                .font(font::BODY)
                .color(p.ink_2)
                .grow(1.0)
                .min_w(0.0)
                .align(rune::Align::End)
                .truncate(),
        )
}

fn connection_row(connection: &Connection, p: &Palette) -> (Color, String) {
    match connection {
        Connection::Checking => (p.ink_3, "Checking...".into()),
        Connection::NotRunning => (p.ink_3, "Not open yet. Start opens it.".into()),
        Connection::NoLmsTab => (p.accent, "Browser open, LMS not open yet".into()),
        Connection::Lms { title, .. } if !title.is_empty() => (p.good, format!("Open on {title}")),
        Connection::Lms { .. } => (p.good, "Open".into()),
    }
}

/// The numbered "How it works" rows, shared by Home and Waiting.
fn how_row(key: &str, number: u8, mark: Mark, title: &str, about: &str, p: &Palette) -> El {
    list_row(key, mark == Mark::Current, p).py(10.0).items_start().child(marker(mark, number, p)).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .pt(2.0)
            .child(text(title.to_string()).key(format!("{key}-title")).font(font::STRONG).color(p.ink))
            .child(text(about.to_string()).key(format!("{key}-about")).font(font::BODY).color(p.ink_2).wrap()),
    )
}

pub fn home(flow: &Flow, p: &Palette) -> El {
    let setup = button("setup", "Setup", Kind::Secondary, true, p, |s: &mut Ui, tx| send(s, Input::OpenSetup, tx));
    let (codex_color, codex_text) = match &flow.codex {
        Codex::Known(CodexStatus::SignedIn(_)) => (p.good, "Signed in".to_string()),
        Codex::Known(_) => (p.warn, "Not signed in. Open Setup to sign in.".to_string()),
        Codex::Checking | Codex::SigningIn => (p.ink_3, "Checking...".to_string()),
    };
    let browser = &flow.browser.setup;
    let (browser_color, browser_text) = match &browser.binary {
        Ok(_) => (p.good, browser.display_name()),
        Err(_) => (p.bad, format!("{} is not installed. Open Setup.", browser.display_name())),
    };
    let (connection_color, connection_text) = connection_row(&flow.connection, p);
    let ready = flow.can_start();
    let status_pill = pill("ready", p)
        .child(if ready { live_dot(p.good, p.good_soft) } else { live_dot(p.warn, p.warn_soft) })
        .child(text(if ready { "Ready" } else { "Needs setup" }).key("ready-label").font(font::STRONG).color(p.ink));

    let start =
        big_button("start", "Start", ready, p, |s: &mut Ui, tx| send(s, Input::Start, tx)).w_full().default_button();
    let opens = match &flow.connection {
        Connection::Lms { .. } | Connection::NoLmsTab => "Uses the browser that is already open.".to_string(),
        _ => format!("Opens {} at the LMS.", browser.display_name()),
    };
    let status = card("status", p)
        .role("status")
        .padding(6.0)
        .gap(2.0)
        .child(status_row("codex-line", codex_color, "ChatGPT", codex_text, p))
        .child(status_row("browser-line", browser_color, "Browser", browser_text, p))
        .child(status_row("connection-line", connection_color, "LMS", connection_text, p));
    let how = col()
        .w_full()
        .gap(2.0)
        .child(how_row("how-1", 1, Mark::Pending, "Press Start", &opens, p))
        .child(how_row(
            "how-2",
            2,
            Mark::Pending,
            "Open the course map or a course",
            "Log in if asked, then open the page you want done. On the course map it works through every unfinished course.",
            p,
        ))
        .child(how_row(
            "how-3",
            3,
            Mark::Pending,
            "Watch it work",
            "Each activity and question shows up here as it is answered. Stop any time.",
            p,
        ));
    let body = col()
        .key("home")
        .gap(space::XL)
        .child(col().w_full().gap(space::SM).child(section("status-head", "Status", None, p)).child(status))
        .child(start)
        .child(hairline(p))
        .child(
            col()
                .w_full()
                .gap(space::SM)
                .child(section("how-head", "How it works", Some(chip("Final tests are never run", p)), p))
                .child(how),
        );
    let header = Header {
        key: "home",
        title: "Ready when you are".into(),
        subtitle: "Press Start and the bot opens your browser, then works through the course page you open.".into(),
        pill: Some(status_pill),
        actions: vec![setup],
    };
    page("Home", header, body, false, p)
}

/// Where the waiting screen is: the three checklist marks, the pill's words, and a notice.
struct WaitView {
    marks: [Mark; 3],
    /// What the third row says about the page the browser is on.
    page: Option<String>,
    pill: &'static str,
    notice: Option<(Tone, String, String)>,
}

fn wait_view(waiting: &Waiting) -> WaitView {
    use Mark::{Current, Done, Failed, Pending};
    let view = |marks, page, pill, notice| WaitView { marks, page, pill, notice };
    let sighting = match waiting {
        Waiting::Opening => return view([Current, Pending, Pending], None, "Opening browser", None),
        Waiting::OpenFailed(e) => {
            let notice = (Tone::Bad, "The browser did not open.".to_string(), e.clone());
            return view([Failed, Pending, Pending], None, "Browser did not open", Some(notice));
        }
        Waiting::Looking(None) => return view([Current, Pending, Pending], None, "Looking at the browser", None),
        Waiting::Looking(Some(sighting)) => sighting,
    };
    match sighting {
        Sighting::NotRunning => view([Current, Pending, Pending], None, "Waiting for the browser", None),
        Sighting::NoLmsTab => view([Done, Current, Pending], None, "Waiting for the LMS", None),
        Sighting::Lms { title, page, .. } => {
            let on = (!title.is_empty()).then(|| format!("On: {title}"));
            match page {
                Some(PageKind::FinalTest) => {
                    let notice =
                        (Tone::Warn, FINAL_TEST_REFUSED.to_string(), "Open another page to continue.".to_string());
                    view([Done, Done, Current], on, "Final test refused", Some(notice))
                }
                _ => view([Done, Done, Current], on, "Waiting for a course", None),
            }
        }
    }
}

pub fn waiting(flow: &Flow, waiting: &Waiting, p: &Palette) -> El {
    let v = wait_view(waiting);
    let cancel = button("cancel", "Cancel", Kind::Secondary, true, p, |s: &mut Ui, tx| send(s, Input::Cancel, tx));
    let lead = match v.notice.as_ref().map(|n| n.0) {
        Some(Tone::Bad) => dot(p.bad),
        Some(Tone::Warn) => dot(p.warn),
        _ => live_dot(p.accent, p.accent_soft),
    };
    let status_pill =
        pill("wait-pill", p).child(lead).child(text(v.pill).key("wait-pill-text").font(font::STRONG).color(p.ink));

    let browser = flow.browser.setup.display_name();
    let check_about = [
        match waiting {
            Waiting::OpenFailed(_) => format!("{browser} could not be opened."),
            _ if v.marks[0] == Mark::Done => format!("{browser} is open."),
            _ => format!("Opening {browser}..."),
        },
        match v.marks[1] {
            Mark::Done => "The LMS is open.".to_string(),
            _ => "Sign in to the Binus LMS in the browser if it asks.".to_string(),
        },
        v.page.clone().unwrap_or_else(|| {
            "The course map, a course, a unit, or an activity. The run starts on its own.".to_string()
        }),
    ];
    let titles = ["Open the browser", "Log in to the LMS", "Open the course map or a course"];
    let mut checklist = card("checklist", p).role("status").padding(6.0).gap(2.0);
    for (i, (title, about)) in titles.iter().zip(check_about).enumerate() {
        checklist = checklist.child(how_row(&format!("check-{}", i + 1), i as u8 + 1, v.marks[i], title, &about, p));
    }

    let mut body = col().key("waiting").gap(space::XL);
    if let Some((tone, title, detail)) = v.notice {
        let action = matches!(waiting, Waiting::OpenFailed(_)).then(|| {
            button("start", "Try again", Kind::Primary, flow.can_start(), p, |s: &mut Ui, tx| send(s, Input::Start, tx))
                .default_button()
        });
        body = body.child(notice(tone, ("wait-status-text", title), Some(("wait-hint", detail)), action, p));
    }
    body =
        body.child(col().w_full().gap(space::SM).child(section("check-head", "Checklist", None, p)).child(checklist));
    let tips = [
        "You don't need to come back here. The run starts as soon as a page it can work on is open.",
        "Keep the browser window open while the bot works.",
        "Final tests are never run, even if you open one.",
    ];
    let mut tip_rows = col().w_full().gap(space::SM).pl(space::MD);
    for (i, tip) in tips.iter().enumerate() {
        tip_rows = tip_rows.child(
            row()
                .key(format!("tip-{i}"))
                .w_full()
                .gap(space::MD)
                .items_start()
                .child(el().pt(2.0).child(glyph(&CHECK, 14.0, p.ink_3)))
                .child(text(tip.to_string()).font(font::BODY).color(p.ink_2).grow(1.0).min_w(0.0).wrap()),
        );
    }
    body = body
        .child(hairline(p))
        .child(col().w_full().gap(space::MD).child(section("tips-head", "Good to know", None, p)).child(tip_rows));

    let header = Header {
        key: "wait",
        title: "Open your course".into(),
        subtitle: WAIT_TEXT.into(),
        pill: Some(status_pill),
        actions: vec![cancel],
    };
    page("Waiting", header, body, false, p)
}
