//! Storage screen: a flat, single-line view of the locally-held inference
//! records. No separators, no cursor glyph — the selected row is highlighted in white.
//!
//! Columns: date (left) · question preview (center, one line) · HIT (%) (right-aligned).
//! In Query Locality mode, HIT (%) is the similarity of each record's question
//! to the user's own past questions, and the list is ordered so the most
//! compatible records sit on top. In Blind Swarm mode the column is omitted and
//! the list is newest-first (no relevance tracking).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::model::{DateFormat, InferenceRecord, ShardingMode};
use crate::theme;

const DATE_W: usize = 16; // "01.01.2026 20:00" is the widest date
const HIT_W: usize = 7;  // "HIT (%)" header + right-aligned "99.99"
const GAP: usize = 2;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let rows =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
            .split(area);

    #[cfg(feature = "publisher")]
    let shortcut_hint = "   ·   p: publish   s: sync web   d: delete";
    #[cfg(not(feature = "publisher"))]
    let shortcut_hint = "   ·   d: delete";

    let indicator = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(
                "{} records · {} GB reserved",
                app.total_records,
                app.settings.reserved_gb
            ),
            theme::muted(),
        ),
        Span::styled(shortcut_hint, theme::muted()),
    ]));
    frame.render_widget(indicator, rows[0]);

    let is_blind = app.settings.sharding_mode == ShardingMode::BlindSwarm;
    frame.render_widget(header_line(rows[1].width, is_blind), rows[1]);

    let fixed = if is_blind {
        DATE_W + GAP
    } else {
        DATE_W + HIT_W + GAP * 2
    };
    let content_w = rows[2].width.saturating_sub(fixed as u16) as usize;

    let viewport_h = rows[2].height as usize;
    // Keep the viewport height so arrow keys only scroll at the edge.
    app.memory_height.set(viewport_h);

    if app.total_records == 0 || viewport_h == 0 {
        return;
    }

    // Relative slicing against the buffered sliding window in app.records:
    let rel_offset = app.memory_offset.saturating_sub(app.records_offset);
    let rel_end = (rel_offset + viewport_h).min(app.records.len());

    let visible_records = if rel_offset < app.records.len() {
        &app.records[rel_offset..rel_end]
    } else {
        &[]
    };

    let items: Vec<ListItem> = visible_records
        .iter()
        .map(|r| row_item(r, app.settings.date_format, content_w, is_blind))
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("");

    let rel_selected = app.memory_selected.saturating_sub(app.memory_offset);
    let mut state = ListState::default()
        .with_selected(Some(rel_selected))
        .with_offset(0);
    frame.render_stateful_widget(list, rows[2], &mut state);
}

/// Column titles: DATE · QUESTION · (HIT (%) if not blind swarm).
fn header_line(width: u16, is_blind: bool) -> Paragraph<'static> {
    let fixed = if is_blind {
        DATE_W + GAP
    } else {
        DATE_W + HIT_W + GAP * 2
    };
    let content_w = width.saturating_sub(fixed as u16) as usize;

    let date = format!("{:<width$}", "DATE", width = DATE_W);
    let question = format!("{:<width$}", "QUESTION", width = content_w);

    if is_blind {
        Paragraph::new(Line::from(vec![
            Span::styled(date, theme::column_header()),
            Span::styled(" ".repeat(GAP), theme::column_header()),
            Span::styled(question, theme::column_header()),
        ]))
    } else {
        let hit = format!("{:>width$}", "HIT (%)", width = HIT_W);
        Paragraph::new(Line::from(vec![
            Span::styled(date, theme::column_header()),
            Span::styled(" ".repeat(GAP), theme::column_header()),
            Span::styled(question, theme::column_header()),
            Span::styled(" ".repeat(GAP), theme::column_header()),
            Span::styled(hit, theme::column_header()),
        ]))
    }
}

fn row_item(
    r: &InferenceRecord,
    fmt: DateFormat,
    content_w: usize,
    is_blind: bool,
) -> ListItem<'static> {
    let date = fmt.format(&r.ts);
    let clean_preview = crate::sanitize::strip_control_chars(r.preview());
    let question = fit(&clean_preview, content_w.saturating_sub(6));

    let web_badge = if r.publish_candidate {
        #[cfg(feature = "publisher")]
        {
            Span::styled("[WEB] ", ratatui::style::Style::default().fg(ratatui::style::Color::Green))
        }
        #[cfg(not(feature = "publisher"))]
        {
            Span::raw("      ")
        }
    } else {
        Span::raw("      ")
    };

    if is_blind {
        ListItem::new(Line::from(vec![
            Span::styled(date, theme::meta()),
            Span::raw(" ".repeat(GAP)),
            web_badge,
            Span::styled(question, theme::content()),
        ]))
    } else {
        // Query Locality: HIT (%) = similarity of this record's question to
        // the user's own past questions.
        let hit = format!("{:>width$}", r.locality_string(), width = HIT_W);
        ListItem::new(Line::from(vec![
            Span::styled(date, theme::meta()),
            Span::raw(" ".repeat(GAP)),
            web_badge,
            Span::styled(question, theme::content()),
            Span::raw(" ".repeat(GAP)),
            Span::styled(hit, theme::meta()),
        ]))
    }
}

/// Pad or ellipsize `s` to exactly `width` columns.
fn fit(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        if width == 0 {
            return String::new();
        }
        if width == 1 {
            return "…".to_string();
        }
        let mut out: String = chars[..width - 1].iter().collect();
        out.push('…');
        out
    } else {
        let mut out = s.to_string();
        while out.chars().count() < width {
            out.push(' ');
        }
        out
    }
}
