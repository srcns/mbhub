//! Rendering entry point. Splits the frame into the green header bar, the
//! active screen's body (or full-screen markdown viewer), and the green footer
//! reference bar.

pub mod footer;
pub mod header;
pub mod markdown;
pub mod memory;
pub mod search;
pub mod settings;
pub mod viewer;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use crate::app::{App, Screen};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let rows = Layout::vertical([
        Constraint::Length(1), // green header bar
        Constraint::Min(0),    // body
        Constraint::Length(1), // green footer bar
    ])
    .split(area);

    header::draw(frame, rows[0], app);

    let body: Rect = rows[1];
    app.body_width.set(body.width as usize);
    app.body_height.set(body.height as usize);

    if let Some(viewer) = &app.viewer {
        viewer::draw(frame, body, viewer);
    } else {
        match app.screen {
            Screen::Search => search::draw(frame, body, app),
            Screen::Memory => memory::draw(frame, body, app),
            Screen::Settings => settings::draw(frame, body, app),
        }
    }

    // Viewer delete confirmation overlay — captures all input while open.
    if let Some(prompt) = &app.delete_prompt {
        if app.viewer.is_some() {
            draw_delete_prompt(frame, body, prompt);
        }
    }

    footer::draw(frame, rows[2], app);
}

/// Centered confirmation overlay for the viewer delete flow.
fn draw_delete_prompt(frame: &mut Frame, area: Rect, prompt: &crate::app::DeletePrompt) {
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};
    use ratatui::text::{Line, Span};

    let height: u16 = if prompt.has_author { 8 } else { 7 };
    let width: u16 = 66.min(area.width.saturating_sub(4));

    let vert = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Fill(1),
        ratatui::layout::Constraint::Length(height),
        ratatui::layout::Constraint::Fill(1),
    ])
    .split(area);
    let horiz = ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Fill(1),
        ratatui::layout::Constraint::Length(width),
        ratatui::layout::Constraint::Fill(1),
    ])
    .split(vert[1]);

    let muted = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(Span::styled(
            "1 · Delete & block THIS content only",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "     permanent local tombstone — survives identity changes",
            muted,
        )),
    ];
    if prompt.has_author {
        lines.push(Line::from(Span::styled(
            "2 · Delete & block EVERYTHING from this publisher",
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(Span::styled(
            "     permanent local ban — their records never return",
            muted,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "   (publisher unknown for this record — content-level only)",
            muted,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Esc · Cancel", muted)));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(" Delete this record? ");
    frame.render_widget(ratatui::widgets::Clear, horiz[1]);
    frame.render_widget(Paragraph::new(lines).block(block), horiz[1]);
}
