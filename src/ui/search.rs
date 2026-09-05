//! Search / main ("Ask") screen. Full-screen, minimal: an empty canvas with a
//! bottom input that soft-wraps and grows upward as lines are added, plus a
//! terse char counter.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, MAX_QUERY_CHARS};
use crate::input::QueryInput;
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    app.search_width.set(width.max(1));

    let nlines = app.search_input.visual_line_count(width).max(1);
    let counter_h = 1u16;
    let max_input = area.height.saturating_sub(counter_h);
    let input_h = (nlines as u16).min(max_input).max(1);

    let rows = Layout::vertical([
        Constraint::Min(0),          // empty canvas (flex)
        Constraint::Length(input_h), // growing, wrapping input
        Constraint::Length(counter_h),
    ])
    .split(area);

    render_input(frame, rows[1], &app.search_input, width);

    let count = app.query_char_count();
    let (label, style) = counter(count);
    let counter = Paragraph::new(Line::from(Span::styled(label, style)))
        .alignment(Alignment::Right);
    frame.render_widget(counter, rows[2]);
}

fn render_input(frame: &mut Frame, area: Rect, input: &QueryInput, width: usize) {
    let cursor_style = Style::default().add_modifier(Modifier::REVERSED);

    if input.is_empty() {
        // Placeholder with the cursor sitting on its first character.
        let ph = "Ask the network ...";
        let mut chars = ph.chars();
        let mut spans = Vec::new();
        if let Some(first) = chars.next() {
            spans.push(Span::styled(first.to_string(), cursor_style));
            spans.push(Span::styled(chars.collect::<String>(), theme::muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let lines = input.visual_text(width);
    let text_style = theme::text();
    for (r, text) in lines.iter().enumerate() {
        if r as u16 >= area.height {
            break;
        }
        let y = area.y + r as u16;
        let para = Paragraph::new(Line::from(Span::styled(text.clone(), text_style)));
        frame.render_widget(para, Rect::new(area.x, y, area.width, 1));
    }

    let (line, col) = input.cursor_line_col(width);
    if (line as u16) < area.height && area.width > 0 {
        let x = area.x + (col as u16).min(area.width - 1);
        let y = area.y + line as u16;
        let sym = input.char_under_cursor();
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(sym, cursor_style))),
            Rect::new(x, y, 1, 1),
        );
    }
}

fn counter(count: usize) -> (String, Style) {
    let style = if count >= MAX_QUERY_CHARS {
        Style::default().fg(theme::DANGER)
    } else if count >= (MAX_QUERY_CHARS * 9) / 10 {
        Style::default().fg(theme::WARN)
    } else {
        theme::muted()
    };
    (format!("{count}/{}", MAX_QUERY_CHARS), style)
}
