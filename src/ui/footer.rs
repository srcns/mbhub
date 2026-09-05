//! Bottom reference bar: a full-width green bar with "tab: switch" (or "esc: back"
//! when reading) on the left and the three screen labels after it.
//!
//! Every label keeps a fixed width; the active label's white background is
//! painted over the *existing* two-space gaps (one column into each side), so
//! switching screens never shifts the labels.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::theme;

const LABELS: [&str; 3] = ["ASK", "MEMORY", "SETTINGS"];

fn index(screen: Screen) -> usize {
    match screen {
        Screen::Search => 0,
        Screen::Memory => 1,
        Screen::Settings => 2,
    }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let active = index(app.screen);
    let base = theme::footer_base();
    let badge = theme::footer_active();

    if app.tos_gate_active {
        let mut spans = Vec::new();
        spans.push(Span::styled(" [ Enter / Y : Accept & Connect ] ", theme::footer_active()));
        spans.push(Span::styled("  ", base));
        spans.push(Span::styled(" [ Esc / Q : Decline & Exit ] ", base));
        spans.push(Span::styled("  ", base));
        spans.push(Span::styled(" ↑/↓/PgUp/PgDn/Space : Scroll agreement ", base));
        let p = Paragraph::new(Line::from(spans)).style(base);
        frame.render_widget(p, area);
        return;
    }

    let hint = if app.viewer.is_some() {
        "esc: back  "
    } else {
        "tab: switch"
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(hint, base));

    for (i, label) in LABELS.iter().enumerate() {
        // The two-space gap before this label is shared with the previous one:
        //   column 1 -> previous label's right extension
        //   column 2 -> this label's left extension
        let prev_is_active = i > 0 && (i - 1) == active;
        let this_is_active = i == active;

        spans.push(Span::styled(" ", if prev_is_active { badge } else { base }));
        spans.push(Span::styled(" ", if this_is_active { badge } else { base }));
        spans.push(Span::styled(
            *label,
            if this_is_active { badge } else { base },
        ));
    }

    // Trailing two-space gap gives the last label a right extension.
    let last_is_active = LABELS.len() - 1 == active;
    spans.push(Span::styled(" ", if last_is_active { badge } else { base }));
    spans.push(Span::styled(" ", base));

    // Fill the remainder of the bar so the green ground spans the full width.
    let used = hint.len()
        + (LABELS.len() + 1) * 2
        + LABELS.iter().map(|l| l.len()).sum::<usize>();
    let width = area.width as usize;

    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), base));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
