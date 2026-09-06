//! Global header: a full-width green bar with "MBHUB" centered in black,
//! and a live status on the far right: the P2P peer count normally, or the
//! transient web-archive sync indicator while a maintainer sync runs.

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

    // Right-side status: the web-archive sync indicator takes priority while
    // active (and for a few seconds after completion); otherwise the live
    // swarm peer count is shown.
    #[cfg(feature = "publisher")]
    let status_text = match app.sync_status {
        Some(crate::app::SyncStatus::Running) => "SYNC: SITE…".to_string(),
        Some(crate::app::SyncStatus::Done { success: true, .. }) => "SYNC: OK".to_string(),
        Some(crate::app::SyncStatus::Done { success: false, .. }) => "SYNC: FAIL".to_string(),
        None => {
            let peers = app
                .p2p
                .as_ref()
                .map(|p| p.connected_peers())
                .unwrap_or(0);
            format!("PEERS: {peers}")
        }
    };

    #[cfg(not(feature = "publisher"))]
    let status_text = {
        let peers = app
            .p2p
            .as_ref()
            .map(|p| p.connected_peers())
            .unwrap_or(0);
        format!("PEERS: {peers}")
    };

    let right_pad = right.saturating_sub(status_text.len());

    let line = Line::from(vec![
        Span::styled(" ".repeat(left), theme::top_header()),
        Span::styled(title, theme::top_header()),
        Span::styled(" ".repeat(right_pad), theme::top_header()),
        Span::styled(status_text, theme::top_header()),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
