//! Full-screen, responsive markdown text viewer with cached line wrapping,
//! provenance metadata bar (Model, Provider, Date), and smooth scrolling.

use std::cell::RefCell;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::model::InferenceRecord;
use crate::theme;
use crate::ui::markdown::render_markdown;

#[derive(Clone, Debug, Default)]
struct ViewerCache {
    width: usize,
    lines: Vec<Line<'static>>,
}

/// Provenance metadata displayed in the top bar of the viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerMetadata {
    pub provider: String,
    pub model: String,
    pub date_str: String,
    /// True when the content arrived from the P2P swarm: brand claims are NOT
    /// verified provenance and must be presented as such.
    pub is_swarm: bool,
    /// True when the viewer is displaying the legal Terms of Service.
    pub is_tos: bool,
}

#[derive(Clone, Debug)]
pub struct ViewerState {
    pub content: String,
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub metadata: Option<ViewerMetadata>,
    pub record: Option<InferenceRecord>,
    cache: RefCell<ViewerCache>,
}

impl ViewerState {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            scroll_offset: 0,
            is_streaming: false,
            metadata: None,
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    pub fn with_record(mut self, record: InferenceRecord) -> Self {
        self.record = Some(record);
        self
    }

    pub fn with_metadata(
        content: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        date_str: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            scroll_offset: 0,
            is_streaming: false,
            metadata: Some(ViewerMetadata {
                provider: provider.into(),
                model: model.into(),
                date_str: date_str.into(),
                is_swarm: false,
                is_tos: false,
            }),
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    /// Viewer for swarm-sourced content: prepends the unverified-source badge
    /// and marks the provenance bar so claimed brands are not shown as verified.
    pub fn with_swarm_metadata(
        content: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        date_str: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let content = format!(
            "> ⚠️ **[SWARM]** Peer-sourced answer — unverified origin. Verify before use.\n\n{content}"
        );
        Self {
            content,
            scroll_offset: 0,
            is_streaming: false,
            metadata: Some(ViewerMetadata {
                provider: provider.into(),
                model: model.into(),
                date_str: date_str.into(),
                is_swarm: true,
                is_tos: false,
            }),
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    /// Viewer for legal agreement (Terms of Service & Legal Framework) text.
    pub fn with_tos_metadata(
        content: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            scroll_offset: 0,
            is_streaming: false,
            metadata: Some(ViewerMetadata {
                provider: "LEGAL".to_string(),
                model: format!("Terms of Service (v{})", version.into()),
                date_str: "Operational Agreement".to_string(),
                is_swarm: false,
                is_tos: true,
            }),
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    #[allow(dead_code)]
    pub fn streaming(initial: impl Into<String>) -> Self {
        Self {
            content: initial.into(),
            scroll_offset: 0,
            is_streaming: true,
            metadata: None,
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    pub fn streaming_with_metadata(
        initial: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        date_str: impl Into<String>,
    ) -> Self {
        Self {
            content: initial.into(),
            scroll_offset: 0,
            is_streaming: true,
            metadata: Some(ViewerMetadata {
                provider: provider.into(),
                model: model.into(),
                date_str: date_str.into(),
                is_swarm: false,
                is_tos: false,
            }),
            record: None,
            cache: RefCell::new(ViewerCache::default()),
        }
    }

    pub fn append_text(&mut self, text: &str) {
        self.content.push_str(text);
        let mut cache = self.cache.borrow_mut();
        cache.width = 0;
        cache.lines.clear();
    }

    /// Recompute word-wrapping if the render width changed.
    pub fn ensure_cached(&self, width: usize) {
        let mut cache = self.cache.borrow_mut();
        if cache.width != width || cache.lines.is_empty() {
            cache.lines = render_markdown(&self.content, width);
            cache.width = width;
        }
    }

    pub fn total_lines(&self, width: usize) -> usize {
        self.ensure_cached(width);
        self.cache.borrow().lines.len()
    }

    pub fn visible_height(&self, body_height: usize) -> usize {
        if self.metadata.is_some() && body_height >= 2 {
            body_height.saturating_sub(1)
        } else {
            body_height
        }
    }

    pub fn max_offset(&self, width: usize, visible_height: usize) -> usize {
        self.total_lines(width).saturating_sub(visible_height)
    }

    pub fn scroll_up(&mut self, delta: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(delta);
    }

    pub fn scroll_down(&mut self, delta: usize, width: usize, visible_height: usize) {
        let max_off = self.max_offset(width, visible_height);
        self.scroll_offset = (self.scroll_offset + delta).min(max_off);
    }

    pub fn scroll_page_up(&mut self, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.scroll_up(step);
    }

    pub fn scroll_page_down(&mut self, width: usize, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.scroll_down(step, width, visible_height);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_bottom(&mut self, width: usize, visible_height: usize) {
        self.scroll_offset = self.max_offset(width, visible_height);
    }
}

pub fn draw(frame: &mut Frame, area: Rect, state: &ViewerState) {
    let (provenance_chunk, content_area) = if state.metadata.is_some() {
        if area.height >= 2 {
            let chunks = Layout::vertical([
                Constraint::Length(1), // Top provenance bar (single-line background strip)
                Constraint::Min(0),    // Markdown content
            ])
            .split(area);

            (Some(chunks[0]), chunks[1])
        } else {
            (None, area)
        }
    } else {
        (None, area)
    };

    let width = content_area.width as usize;
    let height = content_area.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    state.ensure_cached(width);

    let cache = state.cache.borrow();
    let total = cache.lines.len();
    let max_off = total.saturating_sub(height);
    let start = state.scroll_offset.min(max_off);
    let end = (start + height).min(total);

    if let (Some(bar_area), Some(meta)) = (provenance_chunk, &state.metadata) {
        let scroll_info = if total > height {
            Some((start + 1, end, total))
        } else {
            None
        };
        draw_provenance_bar(frame, bar_area, meta, state.is_streaming, scroll_info);
    }

    if start < end {
        let visible_lines: Vec<Line<'static>> = cache.lines[start..end].to_vec();
        let paragraph = Paragraph::new(visible_lines);
        frame.render_widget(paragraph, content_area);
    }
}

fn draw_provenance_bar(
    frame: &mut Frame,
    area: Rect,
    meta: &ViewerMetadata,
    is_streaming: bool,
    scroll_info: Option<(usize, usize, usize)>,
) {
    let base_style = theme::provenance_bar();
    let bold_style = theme::provenance_bar_bold();

    // §5.2: swarm content has no provider attestation — the claimed brand is
    // replaced with an explicit unverified label instead of borrowed trust.
    let provider_label = if meta.is_swarm {
        "Unverified (swarm)"
    } else {
        &meta.provider
    };

    let w = area.width as usize;
    let mut spans = Vec::new();

    if meta.is_tos {
        if w >= 70 {
            spans.push(Span::styled(" AGREEMENT: ", base_style));
            spans.push(Span::styled("MBHub Terms of Service", bold_style));
            spans.push(Span::styled("  │  ", base_style));
            spans.push(Span::styled("DOC: ", base_style));
            spans.push(Span::styled(&meta.model, bold_style));
            spans.push(Span::styled("  │  ", base_style));
            spans.push(Span::styled("STATUS: ", base_style));
            spans.push(Span::styled(&meta.date_str, bold_style));
        } else {
            spans.push(Span::styled(" TERMS OF SERVICE (v1.0.0) ", bold_style));
        }
    } else if w >= 75 {
        // Standard wide bar
        spans.push(Span::styled(" PROVIDER: ", base_style));
        spans.push(Span::styled(provider_label, bold_style));
        spans.push(Span::styled("  │  ", base_style));
        spans.push(Span::styled("MODEL: ", base_style));
        spans.push(Span::styled(&meta.model, bold_style));
        spans.push(Span::styled("  │  ", base_style));
        spans.push(Span::styled("DATE: ", base_style));
        spans.push(Span::styled(&meta.date_str, bold_style));
    } else if w >= 50 {
        // Compact bar for medium width
        let p_short = if meta.is_swarm { "Swarm" } else { &meta.provider };
        spans.push(Span::styled(" P: ", base_style));
        spans.push(Span::styled(p_short, bold_style));
        spans.push(Span::styled(" │ M: ", base_style));
        let m_max = w.saturating_sub(35).max(8);
        let m_disp: String = if meta.model.chars().count() > m_max {
            format!("{}…", &meta.model.chars().take(m_max - 1).collect::<String>())
        } else {
            meta.model.clone()
        };
        spans.push(Span::styled(m_disp, bold_style));
        spans.push(Span::styled(" │ D: ", base_style));
        spans.push(Span::styled(&meta.date_str, bold_style));
    } else {
        // Ultra-compact for narrow width (< 50)
        let m_max = w.saturating_sub(15).max(6);
        let m_disp: String = if meta.model.chars().count() > m_max {
            format!("{}…", &meta.model.chars().take(m_max - 1).collect::<String>())
        } else {
            meta.model.clone()
        };
        spans.push(Span::styled(format!(" {m_disp}"), bold_style));
    }

    if meta.is_swarm && w >= 60 {
        spans.push(Span::styled("  ", base_style));
        spans.push(Span::styled(
            "[SWARM]",
            Style::default()
                .bg(Color::Black)
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if is_streaming {
        spans.push(Span::styled("  ", base_style));
        spans.push(Span::styled(
            if w >= 60 { "[STREAMING ●]" } else { "[●]" },
            Style::default()
                .bg(Color::Black)
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Scroll info on the right: [L start-end/total]
    let scroll_str = scroll_info
        .map(|(s, e, t)| format!(" [L {s}-{e}/{t}]"))
        .unwrap_or_default();

    let used_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let remaining = w.saturating_sub(used_width + scroll_str.len());
    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), base_style));
    }
    if !scroll_str.is_empty() && w >= used_width + scroll_str.len() {
        spans.push(Span::styled(scroll_str, theme::provenance_bar_bold()));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), area);
}
