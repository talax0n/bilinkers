//! The design tokens (colour, type, spacing, radii) and the controls every screen shares.
//!
//! One look: a warm grey window with a single white rounded surface inset in it, hairline
//! dividers, near-black text and a near-black primary button. Dark derives from the same roles.

use rune::tok::{Family, TextStyle};
use rune::{col, el, icon, oklch, oklcha, rich, row, text, Color, Cursor, El, Icon, ScrollSpec, Span};

/// Every colour the window uses, by role.
pub struct Palette {
    /// Behind the surface.
    pub window: Color,
    /// The one content surface, cards and secondary buttons.
    pub surface: Color,
    pub surface_border: Color,
    /// Dividers and the borders of cards and rows.
    pub hairline: Color,
    /// The highlighted row's fill, and quiet fills such as the log.
    pub sunken: Color,
    pub row_border: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub ink_3: Color,
    pub primary: Color,
    pub on_primary: Color,
    pub good: Color,
    pub good_soft: Color,
    pub warn: Color,
    pub warn_soft: Color,
    pub bad: Color,
    pub bad_soft: Color,
    /// In-progress dots.
    pub accent: Color,
    pub accent_soft: Color,
    /// An empty progress bar.
    pub track: Color,
    pub hover: Color,
    pub press: Color,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            window: oklch(0.155, 0.002, 90.0),
            surface: oklch(0.205, 0.002, 90.0),
            surface_border: oklcha(1.0, 0.0, 0.0, 0.08),
            hairline: oklcha(1.0, 0.0, 0.0, 0.075),
            sunken: oklch(0.245, 0.002, 90.0),
            row_border: oklcha(1.0, 0.0, 0.0, 0.1),
            ink: oklch(0.96, 0.0, 0.0),
            ink_2: oklch(0.74, 0.004, 90.0),
            ink_3: oklch(0.56, 0.004, 90.0),
            primary: oklch(0.96, 0.0, 0.0),
            on_primary: oklch(0.18, 0.0, 0.0),
            good: oklch(0.76, 0.14, 152.0),
            good_soft: oklcha(0.76, 0.14, 152.0, 0.12),
            warn: oklch(0.81, 0.12, 78.0),
            warn_soft: oklcha(0.81, 0.12, 78.0, 0.12),
            bad: oklch(0.72, 0.16, 25.0),
            bad_soft: oklcha(0.72, 0.16, 25.0, 0.12),
            accent: oklch(0.72, 0.13, 255.0),
            accent_soft: oklcha(0.72, 0.13, 255.0, 0.12),
            track: oklcha(1.0, 0.0, 0.0, 0.12),
            hover: oklcha(1.0, 0.0, 0.0, 0.04),
            press: oklcha(1.0, 0.0, 0.0, 0.08),
        }
    } else {
        Palette {
            window: oklch(0.945, 0.002, 90.0),
            surface: oklch(1.0, 0.0, 0.0),
            surface_border: oklch(0.91, 0.0, 0.0),
            hairline: oklch(0.925, 0.0, 0.0),
            sunken: oklch(0.972, 0.001, 90.0),
            row_border: oklch(0.905, 0.0, 0.0),
            ink: oklch(0.2, 0.0, 0.0),
            ink_2: oklch(0.5, 0.004, 90.0),
            ink_3: oklch(0.64, 0.004, 90.0),
            primary: oklch(0.2, 0.0, 0.0),
            on_primary: oklch(1.0, 0.0, 0.0),
            good: oklch(0.55, 0.14, 152.0),
            good_soft: oklcha(0.55, 0.14, 152.0, 0.08),
            warn: oklch(0.58, 0.13, 65.0),
            warn_soft: oklcha(0.7, 0.14, 75.0, 0.12),
            bad: oklch(0.55, 0.19, 27.0),
            bad_soft: oklcha(0.55, 0.19, 27.0, 0.07),
            accent: oklch(0.56, 0.17, 258.0),
            accent_soft: oklcha(0.56, 0.17, 258.0, 0.08),
            track: oklch(0.91, 0.0, 0.0),
            hover: oklcha(0.0, 0.0, 0.0, 0.035),
            press: oklcha(0.0, 0.0, 0.0, 0.07),
        }
    }
}

/// Spacing, in points.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    /// Between the window's edge and the surface.
    pub const INSET: f32 = 12.0;
    /// The surface's side padding.
    pub const PAGE_X: f32 = 36.0;
    /// The widest the content gets, so a wide window doesn't stretch rows into a table.
    pub const CONTENT_MAX: f32 = 880.0;
    /// How far a question row sits in from its activity row.
    pub const NEST: f32 = 34.0;
}

pub mod radius {
    pub const SURFACE: f32 = 14.0;
    pub const CARD: f32 = 12.0;
    pub const ROW: f32 = 8.0;
    pub const CONTROL: f32 = 8.0;
    pub const BIG_CONTROL: f32 = 12.0;
}

/// Type.
pub mod font {
    use super::*;

    pub const TITLE: TextStyle = TextStyle::new(24.0, 600.0).lh(30.0).tracking(-0.3);
    pub const SUBTITLE: TextStyle = TextStyle::new(13.5, 400.0).lh(19.0);
    pub const SECTION: TextStyle = TextStyle::new(13.5, 600.0);
    pub const BODY: TextStyle = TextStyle::new(13.0, 400.0).lh(18.0);
    pub const STRONG: TextStyle = TextStyle::new(13.0, 500.0).lh(18.0);
    pub const SMALL: TextStyle = TextStyle::new(12.0, 400.0).lh(16.0);
    pub const SMALL_STRONG: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
    pub const FIGURE: TextStyle = TextStyle::new(34.0, 500.0).lh(40.0).tracking(-0.6).features("'tnum' 1");
    pub const NUMBER: TextStyle = TextStyle::new(12.5, 500.0).features("'tnum' 1");
    pub const BRAND: TextStyle = TextStyle::new(14.0, 600.0);
    pub const BUTTON: TextStyle = TextStyle::new(13.0, 500.0);
    pub const BIG_BUTTON: TextStyle = TextStyle::new(15.0, 600.0);
    pub const MONO: TextStyle = TextStyle::new(11.5, 400.0).family(Family::SystemMono).lh(17.0);
    pub const MONO_LEVEL: TextStyle = TextStyle::new(11.5, 600.0).family(Family::SystemMono).lh(17.0);
}

pub static CHECK: Icon = Icon {
    name: "bilinkers-check",
    view: 16.0,
    body: r#"<path d="M4 8.4L6.7 11L12 5.4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub static CROSS: Icon = Icon {
    name: "bilinkers-cross",
    view: 16.0,
    body: r#"<path d="M5 5L11 11M11 5L5 11" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" fill="none"/>"#,
};
pub static ARROW: Icon = Icon {
    name: "bilinkers-arrow",
    view: 16.0,
    body: r#"<path d="M3.5 8H12.5M8.8 4.3L12.5 8L8.8 11.7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub static CHEVRON_RIGHT: Icon = Icon {
    name: "bilinkers-chevron-right",
    view: 16.0,
    body: r#"<path d="M6.5 4.5L10 8L6.5 11.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub static CHEVRON_DOWN: Icon = Icon {
    name: "bilinkers-chevron-down",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub static RETRY: Icon = Icon {
    name: "bilinkers-retry",
    view: 16.0,
    body: r#"<path d="M12.5 8A4.5 4.5 0 1 1 11.2 4.8M11.6 2.6V5.2H9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub static BANG: Icon = Icon {
    name: "bilinkers-bang",
    view: 16.0,
    body: r#"<path d="M8 4.2V8.8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="8" cy="11.6" r="1.05" fill="currentColor"/>"#,
};
pub static INFO: Icon = Icon {
    name: "bilinkers-info",
    view: 16.0,
    body: r#"<circle cx="8" cy="4.6" r="1.05" fill="currentColor"/><path d="M8 7.3V11.8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};

pub const CREDIT: &str = "Made by TX25-2";

pub fn glyph(i: &'static Icon, size: f32, color: Color) -> El {
    icon(i, size).color(color)
}

pub fn hairline(p: &Palette) -> El {
    el().w_full().h(1.0).fill(p.hairline)
}

/// A status dot.
pub fn dot(color: Color) -> El {
    el().size(8.0, 8.0).radius(4.0).fill(color)
}

/// A dot with a soft halo, for something in progress.
pub fn live_dot(color: Color, halo: Color) -> El {
    el().size(16.0, 16.0).radius(8.0).fill(halo).center().child(dot(color))
}

/// A thin progress bar, `share` of it filled.
pub fn bar(width: f32, share: f32, color: Color, p: &Palette) -> El {
    let filled = (width * share.clamp(0.0, 1.0)).round();
    row().w(width).h(4.0).radius(2.0).fill(p.track).child(el().w(filled).h(4.0).radius(2.0).fill(color))
}

/// Text with its tail in another colour, like a figure whose less important part is grey.
pub fn two_tone(head: &str, tail: &str, style: TextStyle, head_color: Color, tail_color: Color) -> El {
    let content = format!("{head}{tail}");
    rich(content, vec![Span::new(head.len(), head.len() + tail.len()).color(tail_color)]).font(style).color(head_color)
}

/// A step marker: a check when done, the step's number otherwise, filled when it is current.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Done,
    Current,
    Pending,
    Failed,
}

pub fn marker(mark: Mark, number: u8, p: &Palette) -> El {
    let circle = el().size(24.0, 24.0).radius(12.0).center();
    match mark {
        Mark::Done => circle.fill(p.good).child(glyph(&CHECK, 14.0, p.on_primary_fixed())),
        Mark::Failed => circle.fill(p.bad).child(glyph(&CROSS, 14.0, p.on_primary_fixed())),
        Mark::Current => {
            circle.fill(p.primary).child(text(number.to_string()).font(font::SMALL_STRONG).color(p.on_primary))
        }
        Mark::Pending => {
            circle.border(1.0, p.row_border).child(text(number.to_string()).font(font::SMALL_STRONG).color(p.ink_3))
        }
    }
}

impl Palette {
    /// White, for a glyph on a coloured fill in either appearance.
    pub fn on_primary_fixed(&self) -> Color {
        oklch(1.0, 0.0, 0.0)
    }
}

/// A rounded pill with a hairline border, like a status chip.
pub fn pill(key: &str, p: &Palette) -> El {
    row()
        .key(key)
        .role("status")
        .h(32.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .radius(16.0)
        .fill(p.surface)
        .border(1.0, p.row_border)
}

/// A small label chip, such as a question type.
pub fn chip(label: &str, p: &Palette) -> El {
    row()
        .h(20.0)
        .px(7.0)
        .center()
        .radius(6.0)
        .border(1.0, p.row_border)
        .child(text(label.to_string()).font(font::SMALL).color(p.ink_2))
}

/// A section heading with an optional grey note on the right.
pub fn section(key: &str, title: &str, note: Option<El>, p: &Palette) -> El {
    let mut head = row()
        .key(key)
        .w_full()
        .center_y()
        .child(text(title.to_string()).font(font::SECTION).color(p.ink))
        .child(el().grow(1.0));
    if let Some(note) = note {
        head = head.child(note);
    }
    head
}

/// A list row; the current one is highlighted like the reference's "You" row.
pub fn list_row(key: &str, highlighted: bool, p: &Palette) -> El {
    let r = row().key(key).w_full().min_h(44.0).px(space::MD).py(6.0).gap(space::MD).center_y().radius(radius::ROW);
    if highlighted {
        r.fill(p.sunken).border(1.0, p.row_border)
    } else {
        r.border(1.0, Color::TRANSPARENT)
    }
}

/// A bordered card.
pub fn card(key: &str, p: &Palette) -> El {
    col().key(key).w_full().radius(radius::CARD).fill(p.surface).border(1.0, p.row_border)
}

/// The tone of an inline notice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
    Bad,
}

/// A gentle inline notice: an icon, a title and a detail, with an optional action at the end.
pub fn notice(
    tone: Tone,
    title: (&str, String),
    detail: Option<(&str, String)>,
    action: Option<El>,
    p: &Palette,
) -> El {
    let (ink, soft, glyph_icon) = match tone {
        Tone::Info => (p.accent, p.accent_soft, &INFO),
        Tone::Warn => (p.warn, p.warn_soft, &BANG),
        Tone::Bad => (p.bad, p.bad_soft, &BANG),
    };
    let mut words = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(text(title.1).key(title.0.to_string()).font(font::STRONG).color(p.ink).wrap());
    if let Some((key, detail)) = detail {
        words = words.child(text(detail).key(key.to_string()).font(font::BODY).color(p.ink_2).wrap());
    }
    let mut r = row()
        .key("notice")
        .role("status")
        .w_full()
        .padding(14.0)
        .gap(space::MD)
        .radius(radius::CARD)
        .fill(soft)
        .border(1.0, p.row_border)
        .child(el().size(22.0, 22.0).radius(11.0).fill(ink).center().child(glyph(
            glyph_icon,
            14.0,
            p.on_primary_fixed(),
        )))
        .child(words);
    if let Some(action) = action {
        r = r.center_y().child(action);
    }
    r
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The screen's main action: near-black with a trailing arrow.
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
    p: &Palette,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    sized_button(key, label, kind, enabled, (34.0, 14.0, font::BUTTON, radius::CONTROL), p, press)
}

/// The Start button: a full primary button, taller than the rest.
pub fn big_button<S: 'static>(
    key: &str,
    label: &str,
    enabled: bool,
    p: &Palette,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    sized_button(key, label, Kind::Primary, enabled, (52.0, 24.0, font::BIG_BUTTON, radius::BIG_CONTROL), p, press)
}

fn sized_button<S: 'static>(
    key: &str,
    label: &str,
    kind: Kind,
    enabled: bool,
    (height, pad, style, corner): (f32, f32, TextStyle, f32),
    p: &Palette,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let (fill, ink, border) = match kind {
        Kind::Primary => (p.primary, p.on_primary, p.primary),
        Kind::Secondary => (p.surface, p.ink, p.row_border),
    };
    let mut b = row()
        .key(key)
        .role("button")
        .label(label.to_string())
        .h(height)
        .px(pad)
        .gap(6.0)
        .center()
        .radius(corner)
        .fill(fill)
        .border(1.0, border)
        .child(text(label.to_string()).key(format!("{key}-label")).font(style).color(ink));
    if kind == Kind::Primary {
        b = b.child(glyph(&ARROW, style.size + 1.0, ink));
    }
    if !enabled {
        return b.disabled(true).opacity(0.4);
    }
    b.focusable(true)
        .focus_ring(2.0, 2.0, p.accent)
        .hover(p.hover)
        .press(p.press)
        .cursor(Cursor::Pointer)
        .on_press_tx(press)
}

/// A screen's header: its title and one-line subtitle, and on the right a status pill and actions.
pub struct Header {
    /// Keys the title `{key}-title` and the subtitle `{key}-text`.
    pub key: &'static str,
    pub title: String,
    pub subtitle: String,
    pub pill: Option<El>,
    pub actions: Vec<El>,
}

/// The window: the brand on the grey, then one white surface holding the header, a divider,
/// the scrolling body and the credit.
pub fn page(screen: &str, header: Header, body: El, stick_bottom: bool, p: &Palette) -> El {
    let Header { key, title, subtitle, pill, actions } = header;
    let mut right = row().gap(space::SM).center_y();
    if let Some(pill) = pill {
        right = right.child(pill);
    }
    for action in actions {
        right = right.child(action);
    }
    let head = row()
        .key("header")
        .w_full()
        .gap(space::XL)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(text(title).key(format!("{key}-title")).font(font::TITLE).color(p.ink).wrap())
                .child(text(subtitle).key(format!("{key}-text")).font(font::SUBTITLE).color(p.ink_2).wrap()),
        )
        .child(right);
    let content =
        |inner: El| col().w_full().px(space::PAGE_X).child(inner.w_full().max_w(space::CONTENT_MAX).self_center());

    let brand = row()
        .key("brand")
        .w_full()
        .h(28.0)
        .px(6.0)
        .gap(space::SM)
        .center_y()
        .child(
            el().size(20.0, 20.0)
                .radius(6.0)
                .fill(p.primary)
                .center()
                .child(text("B").font(TextStyle::new(12.0, 700.0)).color(p.on_primary)),
        )
        .child(text("Bilinkers").font(font::BRAND).color(p.ink))
        .child(text("/").font(font::BRAND).color(p.ink_3))
        .child(text(screen.to_string()).key("screen").font(TextStyle::new(14.0, 400.0)).color(p.ink_2));

    let footer = row()
        .key("credit")
        .w_full()
        .h(40.0)
        .center_y()
        .child(text(CREDIT).key("credit-text").font(font::SMALL).color(p.ink_3))
        .child(el().grow(1.0))
        .child(text("Bilinkers").font(font::SMALL).color(p.ink_3));

    let surface = col()
        .key("surface")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .radius(radius::SURFACE)
        .fill(p.surface)
        .border(1.0, p.surface_border)
        .clip_rounded(radius::SURFACE)
        .child(content(col().child(head)).pt(28.0).pb(22.0))
        .child(hairline(p))
        .child(
            col()
                .key("body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom })
                .child(content(body).pt(space::XL).pb(space::XXL)),
        )
        .child(hairline(p))
        .child(content(footer));

    col().w_full().h_full().padding(space::INSET).gap(10.0).fill(p.window).child(brand).child(surface)
}
