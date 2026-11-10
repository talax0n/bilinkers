//! The colours, type and controls every screen shares.

use rune::tok::{self, type_, Family, TextStyle};
use rune::{col, el, oklch, text, Color, Cursor, El};

/// The colours the theme tokens don't have, tuned per appearance for contrast on `bg` and `card`.
pub struct Tones {
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub accent: Color,
    /// Text on an accent fill.
    pub on_accent: Color,
}

pub fn tones(dark: bool) -> Tones {
    if dark {
        Tones {
            good: oklch(0.78, 0.13, 150.0),
            warn: oklch(0.82, 0.12, 80.0),
            bad: oklch(0.72, 0.16, 25.0),
            accent: tok::accent(0, true),
            on_accent: oklch(0.99, 0.0, 0.0),
        }
    } else {
        Tones {
            good: oklch(0.5, 0.13, 150.0),
            warn: oklch(0.55, 0.12, 65.0),
            bad: oklch(0.52, 0.18, 27.0),
            accent: oklch(0.5, 0.15, 256.0),
            on_accent: oklch(0.99, 0.0, 0.0),
        }
    }
}

pub const MONO: TextStyle = TextStyle::new(12.0, 400.0).family(Family::SystemMono).lh(17.0);
pub const MONO_LEVEL: TextStyle = TextStyle::new(12.0, 600.0).family(Family::SystemMono).lh(17.0);
pub const FIGURE: TextStyle = TextStyle::new(22.0, 600.0).features("'tnum' 1");
pub const HEADING: TextStyle = TextStyle::new(15.0, 600.0);
pub const SUBHEADING: TextStyle = TextStyle::new(15.0, 400.0);
pub const TITLE: TextStyle = TextStyle::new(20.0, 600.0);
pub const STEP_TITLE: TextStyle = TextStyle::new(14.0, 600.0);
const BUTTON: TextStyle = TextStyle::new(13.0, 500.0);
const BIG_BUTTON: TextStyle = TextStyle::new(16.0, 600.0);

/// A bordered card, the surface every block sits on.
pub fn card(key: &str, t: &tok::Theme) -> El {
    col().key(key).padding(12.0).radius(8.0).fill(t.card).border(1.0, t.card_border)
}

/// A small caps-free section title inside a card.
pub fn section(label: &str, t: &tok::Theme) -> El {
    text(label.to_string()).font(type_::LABEL).color(t.text_secondary)
}

/// A status dot and its text in one colour.
pub fn dot(color: Color) -> El {
    el().size(8.0, 8.0).radius(4.0).fill(color)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The screen's main action, accent filled.
    Primary,
    Secondary,
    /// Text only, for a small side action such as Setup.
    Quiet,
}

/// A done step's check mark or a pending step's number, in a circle.
pub fn badge(done: bool, number: u8, t: &tok::Theme, tones: &Tones) -> El {
    let circle = el().size(22.0, 22.0).radius(11.0).center();
    if done {
        return circle
            .fill(tones.good)
            .child(text("\u{2713}").font(TextStyle::new(13.0, 700.0)).color(tones.on_accent));
    }
    circle.border(1.5, t.control_off).child(text(number.to_string()).font(type_::LABEL).color(t.text_secondary))
}

/// A push button: focusable, pressed by click, Space, or Return when it is the default.
/// A disabled one is dimmed and has no handler, so it can't be focused or pressed.
pub fn button<S: 'static>(
    key: &str,
    label: &str,
    kind: Kind,
    enabled: bool,
    t: &tok::Theme,
    tones: &Tones,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    sized_button(key, label, kind, enabled, (30.0, 14.0, BUTTON), t, tones, press)
}

/// The Start button: a primary button twice the usual height.
pub fn big_button<S: 'static>(
    key: &str,
    label: &str,
    enabled: bool,
    t: &tok::Theme,
    tones: &Tones,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    sized_button(key, label, Kind::Primary, enabled, (48.0, 40.0, BIG_BUTTON), t, tones, press).radius(10.0)
}

#[allow(clippy::too_many_arguments)]
fn sized_button<S: 'static>(
    key: &str,
    label: &str,
    kind: Kind,
    enabled: bool,
    (height, pad, font): (f32, f32, TextStyle),
    t: &tok::Theme,
    tones: &Tones,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let (fill, ink, border) = match kind {
        Kind::Primary => (tones.accent, tones.on_accent, tones.accent),
        Kind::Secondary => (t.card, t.text, t.separator),
        Kind::Quiet => (Color::TRANSPARENT, t.text_secondary, Color::TRANSPARENT),
    };
    let pad = if kind == Kind::Quiet { 8.0 } else { pad };
    let b = el()
        .key(key)
        .role("button")
        .label(label.to_string())
        .h(height)
        .px(pad)
        .center()
        .radius(6.0)
        .fill(fill)
        .border(1.0, border)
        .child(text(label.to_string()).key(format!("{key}-label")).font(font).color(ink));
    if !enabled {
        return b.disabled(true).opacity(0.45);
    }
    b.focusable(true)
        .focus_ring(2.0, 2.0, tones.accent)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .on_press_tx(press)
}
