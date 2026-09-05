//! Central color/style tokens. The accent is green across the whole UI: the
//! top header and the bottom reference bar are green bars, the active footer
//! item is a white badge.

use ratatui::style::{Color, Modifier, Style};

// Brand / structural colors
pub const ACCENT: Color = Color::Green;

// Text colors
pub const TEXT: Color = Color::Reset; // terminal default foreground
pub const MUTED: Color = Color::DarkGray;
pub const META: Color = Color::Gray; // dates + similarity share this color

// Selection
pub const SELECT_BG: Color = Color::White;
pub const SELECT_FG: Color = Color::Black;

// Counter / states
pub const WARN: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;

pub fn accent() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn text() -> Style {
    Style::default().fg(TEXT)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

/// Date + similarity columns.
pub fn meta() -> Style {
    Style::default().fg(META)
}

/// Content text column (different color from the meta columns).
pub fn content() -> Style {
    Style::default().fg(TEXT)
}

pub fn selected() -> Style {
    Style::default().bg(SELECT_BG).fg(SELECT_FG)
}

pub fn focus() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Top header bar: green ground, black text.
pub fn top_header() -> Style {
    Style::default()
        .bg(ACCENT)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD)
}

/// Footer reference bar base: green ground, black text.
pub fn footer_base() -> Style {
    Style::default().bg(ACCENT).fg(Color::Black)
}

/// Active screen label in the footer: white badge, black text.
pub fn footer_active() -> Style {
    Style::default()
        .bg(Color::White)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD)
}

/// Storage column-header bar: white ground, black text.
pub fn column_header() -> Style {
    Style::default().bg(Color::White).fg(Color::Black)
}

/// Detail viewer provenance banner: green ground, black text (matching the header).
pub fn provenance_bar() -> Style {
    Style::default().bg(ACCENT).fg(Color::Black)
}

/// Detail viewer provenance banner bold values.
pub fn provenance_bar_bold() -> Style {
    Style::default()
        .bg(ACCENT)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD)
}
