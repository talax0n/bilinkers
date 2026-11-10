//! The colours, type and controls both screens share.

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
const BUTTON: TextStyle = TextStyle::new(13.0, 500.0);

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
    let (fill, ink, border) = match kind {
        Kind::Primary => (tones.accent, tones.on_accent, tones.accent),
        Kind::Secondary => (t.card, t.text, t.separator),
    };
    let b = el()
        .key(key)
        .role("button")
        .label(label.to_string())
        .h(30.0)
        .px(14.0)
        .center()
        .radius(6.0)
        .fill(fill)
        .border(1.0, border)
        .child(text(label.to_string()).key(format!("{key}-label")).font(BUTTON).color(ink));
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
