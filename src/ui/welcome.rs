//! The welcome screen: sign in with ChatGPT through Codex, choose the browser, Continue.

use rune::{col, el, row, text, Color, Cursor, El};

use crate::setup::{BrowserPick, CodexStatus, Origin};
use crate::ui::flow::{Codex, Flow, Input};
use crate::ui::style::{
    bar, button, card, dot, font, hairline, marker, page, pill, space, Header, Kind, Mark, Palette, CHECK,
};
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

/// A step's heading: its key, number, mark, title and what it is for.
struct Step<'a> {
    key: &'a str,
    number: u8,
    mark: Mark,
    title: &'a str,
    about: &'a str,
}

/// A step card: marker, title and description, an optional action on the right, then `body`
/// under a hairline.
fn step(Step { key, number, mark, title, about }: Step, action: Option<El>, body: El, p: &Palette) -> El {
    let mut head = row().w_full().padding(18.0).gap(14.0).center_y().child(marker(mark, number, p)).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .child(text(title.to_string()).key(format!("{key}-title")).font(font::SECTION).color(p.ink))
            .child(text(about.to_string()).font(font::BODY).color(p.ink_2).wrap()),
    );
    if let Some(action) = action {
        head = head.child(action);
    }
    card(key, p).child(head).child(hairline(p)).child(col().w_full().padding(10.0).gap(6.0).child(body))
}

fn status_line(key: &str, color: Color, message: String, p: &Palette) -> El {
    row()
        .key(key)
        .role("status")
        .w_full()
        .px(8.0)
        .py(6.0)
        .gap(10.0)
        .center_y()
        .child(dot(color))
        .child(text(message).key(format!("{key}-text")).font(font::BODY).color(p.ink).grow(1.0).min_w(0.0).wrap())
}

fn codex_step(flow: &Flow, p: &Palette) -> El {
    let sign_in = |enabled: bool, label: &str| {
        button("sign-in", label, Kind::Primary, enabled, p, |s: &mut Ui, tx| send(s, Input::SignIn, tx))
    };
    let (color, message, action) = match &flow.codex {
        Codex::Checking => (p.ink_3, "Checking your sign-in...".to_string(), None),
        Codex::SigningIn => (p.accent, SIGNING_IN.to_string(), Some(sign_in(false, "Signing in"))),
        Codex::Known(CodexStatus::SignedIn(line)) => (p.good, signed_in_text(line), None),
        Codex::Known(CodexStatus::SignedOut) => {
            (p.ink_3, "Not signed in yet.".to_string(), Some(sign_in(true, "Sign in with ChatGPT")))
        }
        Codex::Known(CodexStatus::Missing) => {
            let retry = button("retry-codex", "Retry", Kind::Secondary, true, p, |s: &mut Ui, tx| {
                send(s, Input::CheckCodex, tx)
            });
            (p.warn, CODEX_INSTALL.to_string(), Some(retry))
        }
        Codex::Known(CodexStatus::Failed(e)) => {
            (p.warn, format!("Could not check your sign-in: {e}"), Some(sign_in(true, "Sign in with ChatGPT")))
        }
    };
    let mut body = col().w_full().gap(6.0).child(status_line("codex-status", color, message, p));
    if let Some(e) = &flow.codex_error {
        body = body.child(
            text(format!("Sign-in did not finish: {e}"))
                .key("codex-error")
                .font(font::BODY)
                .color(p.bad)
                .px(8.0)
                .wrap(),
        );
    }
    let mark = if flow.signed_in() { Mark::Done } else { Mark::Current };
    step(
        Step {
            key: "codex",
            number: 1,
            mark,
            title: "Sign in with ChatGPT",
            about: "The bot answers some questions with your ChatGPT account, through Codex.",
        },
        action,
        body,
        p,
    )
}

/// One browser as a selectable row: its number, a radio mark, its name, and where it is.
fn choice(key: String, number: usize, label: String, path: String, selected: bool, input: Input, p: &Palette) -> El {
    let mark = if selected {
        el().size(18.0, 18.0).radius(9.0).fill(p.primary).center().child(rune::icon(&CHECK, 13.0).color(p.on_primary))
    } else {
        el().size(18.0, 18.0).radius(9.0).border(1.0, p.row_border).fill(p.surface)
    };
    let mut r = row()
        .key(key.clone())
        .role("radio")
        .label(label.clone())
        .checked(selected)
        .focusable(true)
        .focus_selected(selected)
        .focus_ring(2.0, 2.0, p.accent)
        .w_full()
        .min_w(0.0)
        .h(52.0)
        .px(space::MD)
        .gap(space::MD)
        .center_y()
        .radius(8.0)
        .fill(if selected { p.sunken } else { Color::TRANSPARENT })
        .border(1.0, if selected { p.row_border } else { Color::TRANSPARENT })
        .hover(p.hover)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |s: &mut Ui, tx| send(s, input.clone(), tx))
        .child(text(number.to_string()).font(font::NUMBER).color(p.ink_3).w(14.0))
        .child(mark)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(text(label).font(font::STRONG).color(p.ink).truncate())
                .child(text(path).key(format!("{key}-path")).font(font::SMALL).color(p.ink_3).truncate()),
        );
    if selected {
        r = r.child(text("Selected").font(font::SMALL_STRONG).color(p.ink_2));
    }
    r
}

fn origin_text(origin: Origin) -> &'static str {
    match origin {
        Origin::Process => "an environment variable",
        Origin::DotEnv | Origin::Settings => "your .env file",
    }
}

fn browser_step(flow: &Flow, p: &Palette) -> El {
    let b = &flow.browser;
    let mut body = col().w_full().gap(6.0);
    let mut action = None;
    if let Some(origin) = b.locked_by {
        body = match &b.setup.binary {
            Ok(_) => body.child(status_line(
                "browser-status",
                p.good,
                format!("{}, chosen in {}.", b.setup.display_name(), origin_text(origin)),
                p,
            )),
            Err(_) => body.child(status_line(
                "browser-status",
                p.bad,
                format!(
                    "{} picks {}, which isn't installed. Change BROWSER there, or remove it to choose here.",
                    origin_text(origin),
                    b.setup.display_name()
                ),
                p,
            )),
        };
    } else {
        let mut choices =
            col().key("browsers").role("radiogroup").label("Browser").focus_group().focus_follows().w_full().gap(2.0);
        for (i, found) in b.installed.iter().enumerate() {
            let kind = found.kind;
            choices = choices.child(choice(
                format!("browser-{}", kind.key()),
                i + 1,
                kind.label().to_string(),
                found.path.display().to_string(),
                b.pick == Some(BrowserPick::Installed(kind)),
                Input::PickBrowser(kind),
                p,
            ));
        }
        if let Some(BrowserPick::File(path)) = &b.pick {
            let name =
                path.file_stem().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned());
            let shown = path.display().to_string();
            let number = b.installed.len() + 1;
            choices = choices.child(choice("browser-file".into(), number, name, shown, true, Input::ChooseFile, p));
        }
        if b.installed.is_empty() {
            body = body.child(status_line("browser-status", p.warn, NONE_FOUND.to_string(), p));
            action = Some(button("choose-file", "Choose file...", Kind::Secondary, true, p, |s: &mut Ui, tx| {
                send(s, Input::ChooseFile, tx)
            }));
        }
        if b.installed.len() + usize::from(matches!(b.pick, Some(BrowserPick::File(_)))) > 0 {
            body = body.child(choices);
        }
    }
    if let Some(e) = &b.error {
        body = body.child(text(e.clone()).key("browser-error").font(font::BODY).color(p.bad).px(8.0).wrap());
    }
    let mark = match (b.done(), flow.signed_in()) {
        (true, _) => Mark::Done,
        (false, true) => Mark::Current,
        (false, false) => Mark::Pending,
    };
    step(
        Step {
            key: "browser",
            number: 2,
            mark,
            title: "Choose your browser",
            about: "The bot opens the LMS in this browser and works in it.",
        },
        action,
        body,
        p,
    )
}

pub fn view(flow: &Flow, p: &Palette) -> El {
    let done = usize::from(flow.signed_in()) + usize::from(flow.browser.done());
    let progress = pill("setup-progress", p)
        .child(text(format!("{done} of 2 done")).key("setup-progress-text").font(font::STRONG).color(p.ink))
        .child(bar(56.0, done as f32 / 2.0, if done == 2 { p.good } else { p.primary }, p));
    let go = button("continue", "Continue", Kind::Primary, flow.welcome_done(), p, |s: &mut Ui, tx| {
        send(s, Input::Continue, tx)
    })
    .default_button();
    let footer = row()
        .key("footer")
        .w_full()
        .gap(space::MD)
        .center_y()
        .child(
            text("You can change these later from Setup on the home screen.")
                .font(font::BODY)
                .color(p.ink_3)
                .grow(1.0)
                .min_w(0.0)
                .wrap(),
        )
        .child(go);
    let body = col()
        .key("steps")
        .gap(space::LG)
        .child(codex_step(flow, p))
        .child(browser_step(flow, p))
        .child(el().h(space::XS))
        .child(footer);
    let header = Header {
        key: "welcome",
        title: "Set up Bilinkers".into(),
        subtitle: "Two quick steps. You only do this once.".into(),
        pill: Some(progress),
        actions: vec![],
    };
    page("Welcome", header, body, false, p)
}
