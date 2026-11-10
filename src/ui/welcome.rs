//! The welcome screen: sign in with ChatGPT through Codex, choose the browser, Continue.

use rune::tok::{self, type_};
use rune::{col, el, row, text, Color, Cursor, El, ScrollSpec};

use crate::setup::{BrowserPick, CodexStatus, Origin};
use crate::ui::flow::{Codex, Flow, Input};
use crate::ui::style::{badge, button, card, dot, Kind, Tones, HEADING, SMALL, STEP_TITLE, SUBHEADING, TITLE};
use crate::ui::{send, Ui};

pub const CODEX_INSTALL: &str =
    "Codex is not installed. Install it with npm i -g @openai/codex (or from the Codex CLI page), then press Retry.";
pub const SIGNING_IN: &str = "Waiting for sign-in in your browser...";
pub const NONE_FOUND: &str =
    "Searched this computer for Google Chrome, Brave, Microsoft Edge and Chromium and found none. Choose the browser program instead.";

/// "Signed in with ChatGPT" for a ChatGPT login, the CLI's own words otherwise.
pub fn signed_in_text(line: &str) -> String {
    if line.contains("ChatGPT") {
        "Signed in with ChatGPT".to_string()
    } else {
        line.to_string()
    }
}

/// A step's heading: its key, number, whether it is done, its title and what it is for.
struct Step<'a> {
    key: &'a str,
    number: u8,
    done: bool,
    title: &'a str,
    about: &'a str,
}

/// A step: its badge, title and description, an optional action on the right, then `body`.
fn step(
    Step { key, number, done, title, about }: Step,
    action: Option<El>,
    body: El,
    t: &tok::Theme,
    tones: &Tones,
) -> El {
    let mut head = row().w_full().gap(12.0).center_y().child(badge(done, number, t, tones)).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .child(text(title.to_string()).key(format!("{key}-title")).font(STEP_TITLE).color(t.text))
            .child(text(about.to_string()).font(type_::BODY).color(t.text_secondary).wrap()),
    );
    if let Some(action) = action {
        head = head.child(action);
    }
    card(key, t).w_full().padding(16.0).gap(12.0).child(head).child(col().w_full().pl(34.0).gap(6.0).child(body))
}

fn status_line(key: &str, color: Color, message: String, t: &tok::Theme) -> El {
    row()
        .key(key)
        .role("status")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(dot(color))
        .child(text(message).key(format!("{key}-text")).font(type_::BODY).color(t.text).grow(1.0).min_w(0.0).wrap())
}

fn codex_step(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let sign_in = |enabled: bool, label: &str| {
        button("sign-in", label, Kind::Primary, enabled, t, tones, |s: &mut Ui, tx| send(s, Input::SignIn, tx))
    };
    let (color, message, action) = match &flow.codex {
        Codex::Checking => (t.text_secondary, "Checking your sign-in...".to_string(), None),
        Codex::SigningIn => (tones.accent, SIGNING_IN.to_string(), Some(sign_in(false, "Signing in"))),
        Codex::Known(CodexStatus::SignedIn(line)) => (tones.good, signed_in_text(line), None),
        Codex::Known(CodexStatus::SignedOut) => {
            (t.text_secondary, "Not signed in yet.".to_string(), Some(sign_in(true, "Sign in with ChatGPT")))
        }
        Codex::Known(CodexStatus::Missing) => {
            let retry = button("retry-codex", "Retry", Kind::Secondary, true, t, tones, |s: &mut Ui, tx| {
                send(s, Input::CheckCodex, tx)
            });
            (tones.warn, CODEX_INSTALL.to_string(), Some(retry))
        }
        Codex::Known(CodexStatus::Failed(e)) => {
            (tones.warn, format!("Could not check your sign-in: {e}"), Some(sign_in(true, "Sign in with ChatGPT")))
        }
    };
    let mut body = col().w_full().gap(6.0).child(status_line("codex-status", color, message, t));
    if let Some(e) = &flow.codex_error {
        body = body.child(
            text(format!("Sign-in did not finish: {e}")).key("codex-error").font(type_::BODY).color(tones.bad).wrap(),
        );
    }
    step(
        Step {
            key: "codex",
            number: 1,
            done: flow.signed_in(),
            title: "Sign in with ChatGPT",
            about: "The bot answers some questions with your ChatGPT account, through Codex.",
        },
        action,
        body,
        t,
        tones,
    )
}

/// One browser as a radio card: its name, and below it where it is.
fn choice(key: String, label: String, path: String, selected: bool, input: Input, t: &tok::Theme, tones: &Tones) -> El {
    let mark = if selected {
        el().size(16.0, 16.0).radius(8.0).border(5.0, tones.accent).fill(tones.on_accent)
    } else {
        el().size(16.0, 16.0).radius(8.0).border(1.0, t.control_off).fill(t.card)
    };
    row()
        .key(key.clone())
        .role("radio")
        .label(label.clone())
        .checked(selected)
        .focusable(true)
        .focus_selected(selected)
        .focus_ring(2.0, 2.0, tones.accent)
        .w_full()
        .min_w(0.0)
        .h(48.0)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .radius(8.0)
        .fill(if selected { t.press } else { t.bg })
        .border(if selected { 1.5 } else { 1.0 }, if selected { tones.accent } else { t.separator })
        .hover(t.hover)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |s: &mut Ui, tx| send(s, input.clone(), tx))
        .child(mark)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(text(label).font(type_::TITLE).color(t.text).truncate())
                .child(text(path).key(format!("{key}-path")).font(SMALL).color(t.text_secondary).truncate()),
        )
}

fn origin_text(origin: Origin) -> &'static str {
    match origin {
        Origin::Process => "an environment variable",
        Origin::DotEnv | Origin::Settings => "your .env file",
    }
}

fn browser_step(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let b = &flow.browser;
    let mut body = col().w_full().gap(8.0);
    let mut action = None;
    if let Some(origin) = b.locked_by {
        body = match &b.setup.binary {
            Ok(_) => body.child(status_line(
                "browser-status",
                tones.good,
                format!("{}, chosen in {}.", b.setup.display_name(), origin_text(origin)),
                t,
            )),
            Err(_) => body.child(status_line(
                "browser-status",
                tones.bad,
                format!(
                    "{} picks {}, which isn't installed. Change BROWSER there, or remove it to choose here.",
                    origin_text(origin),
                    b.setup.display_name()
                ),
                t,
            )),
        };
    } else {
        let mut choices =
            col().key("browsers").role("radiogroup").label("Browser").focus_group().focus_follows().w_full().gap(6.0);
        for found in &b.installed {
            let kind = found.kind;
            choices = choices.child(choice(
                format!("browser-{}", kind.key()),
                kind.label().to_string(),
                found.path.display().to_string(),
                b.pick == Some(BrowserPick::Installed(kind)),
                Input::PickBrowser(kind),
                t,
                tones,
            ));
        }
        if let Some(BrowserPick::File(path)) = &b.pick {
            let name =
                path.file_stem().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned());
            let shown = path.display().to_string();
            choices = choices.child(choice("browser-file".into(), name, shown, true, Input::ChooseFile, t, tones));
        }
        if b.installed.is_empty() {
            body = body.child(status_line("browser-status", tones.warn, NONE_FOUND.to_string(), t));
            action =
                Some(button("choose-file", "Choose file...", Kind::Secondary, true, t, tones, |s: &mut Ui, tx| {
                    send(s, Input::ChooseFile, tx)
                }));
        }
        if b.installed.len() + usize::from(matches!(b.pick, Some(BrowserPick::File(_)))) > 0 {
            body = body.child(choices);
        }
    }
    if let Some(e) = &b.error {
        body = body.child(text(e.clone()).key("browser-error").font(type_::BODY).color(tones.bad).wrap());
    }
    step(
        Step {
            key: "browser",
            number: 2,
            done: b.done(),
            title: "Choose your browser",
            about: "The bot opens the LMS in this browser and works in it.",
        },
        action,
        body,
        t,
        tones,
    )
}

pub fn view(flow: &Flow, t: &tok::Theme, tones: &Tones) -> El {
    let header = row()
        .key("header")
        .w_full()
        .center_y()
        .gap(8.0)
        .child(text("Beelingua Bot").font(HEADING).color(t.text))
        .child(text("Welcome").key("screen").font(SUBHEADING).color(t.text_secondary));
    let intro = col()
        .gap(4.0)
        .child(text("Let's get you set up").key("welcome-title").font(TITLE).color(t.text))
        .child(text("Two quick steps. You only need to do this once.").font(type_::BODY).color(t.text_secondary));
    let go = button("continue", "Continue", Kind::Primary, flow.welcome_done(), t, tones, |s: &mut Ui, tx| {
        send(s, Input::Continue, tx)
    })
    .default_button();
    let footer = row().key("footer").w_full().center_y().child(el().grow(1.0)).child(go);
    let steps = col()
        .key("steps")
        .w_full()
        .max_w(640.0)
        .self_center()
        .gap(12.0)
        .child(intro)
        .child(codex_step(flow, t, tones))
        .child(browser_step(flow, t, tones))
        .child(footer);
    col()
        .key("welcome")
        .w_full()
        .h_full()
        .gap(16.0)
        .child(header)
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(steps))
}
