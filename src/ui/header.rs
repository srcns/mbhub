//! Global header: a full-width green bar with "MBHUB" centered in black,
//! and the live P2P peer count on the far right.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let title = "MBHUB";
    let left = width.saturating_sub(title.len()) / 2;
    let right = width.saturating_sub(left + title.len());

    // Live swarm status: how many peers this node is connected to right now.
    let peers = app
        .p2p
        .as_ref()
        .map(|p| p.connected_peers())
        .unwrap_or(0);
    let peers_text = format!("PEERS: {peers}");
    let right_pad = right.saturating_sub(peers_text.len());

    let line = Line::from(vec![
        Span::styled(" ".repeat(left), theme::top_header()),
        Span::styled(title, theme::top_header()),
        Span::styled(" ".repeat(right_pad), theme::top_header()),
        Span::styled(peers_text, theme::top_header()),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
