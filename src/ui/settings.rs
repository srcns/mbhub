//! Settings screen. Flat BIOS-like list with clear, uncluttered typography,
//! dynamic model discovery visibility, hierarchical categories, and modal selection pickers.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, PickerModal, SettingsField};
use crate::model::PROVIDERS;
use crate::theme;

const LABEL_W: u16 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SettingItem {
    Field(SettingsField),
    ApiEndpoint,
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let s = &app.settings;
    let focus = app.focus;
    let editing = app.editing;
    let visible_fields = app.visible_fields();

    let mut all_items: Vec<SettingItem> = Vec::new();
    for &field in &visible_fields {
        all_items.push(SettingItem::Field(field));
        if field == SettingsField::Provider && app.provider_models.is_empty() {
            all_items.push(SettingItem::ApiEndpoint);
        } else if field == SettingsField::ProviderModel {
            all_items.push(SettingItem::ApiEndpoint);
        }
    }

    let focused_idx = all_items
        .iter()
        .position(|item| matches!(item, SettingItem::Field(f) if *f == focus))
        .unwrap_or(0);

    let total_h = area.height as usize;
    let width = (area.width as usize).max(10);

    // Calculate required help box height so description and hint are never clipped
    let (_, description, hint) = field_help(focus, editing);
    let desc_lines = wrap_description(description, width);
    let hint_lines = wrap_description(&format!("💡 {hint}"), width);
    let needed_help_lines = 1 + desc_lines.len() + hint_lines.len();

    // Determine layout: fields area vs help area
    // An ideal layout gives 1 line top-pad, all items, 1 line blank separator, and full help box
    let ideal_h = 1 + all_items.len() + 1 + needed_help_lines;

    let (help_h, fields_h, with_blank_line) = if total_h >= ideal_h {
        let help_area_h = (total_h - (1 + all_items.len())).max(needed_help_lines);
        (help_area_h, 1 + all_items.len(), true)
    } else {
        // Space is constrained: reserve at least 3-4 lines for fields to allow navigation,
        // and allocate what help needs up to total_h - 3
        let max_help = total_h.saturating_sub(4).max(2);
        let help_area_h = needed_help_lines.min(max_help);
        let remaining_fields = total_h.saturating_sub(help_area_h);
        (help_area_h, remaining_fields, false)
    };

    // Build constraints: top padding + visible field rows + help area
    let has_top_pad = fields_h > all_items.len() || fields_h > 4;
    let max_field_rows = if has_top_pad {
        fields_h.saturating_sub(1)
    } else {
        fields_h
    };
    let visible_count = max_field_rows.min(all_items.len());

    let (start_idx, end_idx) = if visible_count >= all_items.len() {
        (0, all_items.len())
    } else {
        let half = visible_count / 2;
        let start = if focused_idx < half {
            0
        } else if focused_idx + (visible_count - half) > all_items.len() {
            all_items.len().saturating_sub(visible_count)
        } else {
            focused_idx.saturating_sub(half)
        };
        (start, (start + visible_count).min(all_items.len()))
    };

    let mut constraints = Vec::new();
    if has_top_pad {
        constraints.push(Constraint::Length(1));
    }
    for _ in start_idx..end_idx {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(help_h as u16));

    let rows = Layout::vertical(constraints).split(area);

    let mut row_idx = if has_top_pad { 1 } else { 0 };
    for &item in &all_items[start_idx..end_idx] {
        if row_idx >= rows.len().saturating_sub(1) {
            break;
        }
        match item {
            SettingItem::Field(field) => match field {
                SettingsField::DateFormat => {
                    row(
                        frame,
                        rows[row_idx],
                        "Date format",
                        s.date_format.label(),
                        focus == SettingsField::DateFormat,
                        false,
                    );
                }
                SettingsField::Storage => {
                    if editing && focus == SettingsField::Storage {
                        storage_edit_row(frame, rows[row_idx], app);
                    } else {
                        row(
                            frame,
                            rows[row_idx],
                            "Reserved storage",
                            &format!("{} GB", s.reserved_gb),
                            focus == SettingsField::Storage,
                            false,
                        );
                    }
                }
                SettingsField::ShardingMode => {
                    row(
                        frame,
                        rows[row_idx],
                        "Sharding mode",
                        s.sharding_mode.label(),
                        focus == SettingsField::ShardingMode,
                        false,
                    );
                }
                SettingsField::HitRate => {
                    row(
                        frame,
                        rows[row_idx],
                        "Hit rate threshold",
                        s.hit_rate.label(),
                        focus == SettingsField::HitRate,
                        false,
                    );
                }
                SettingsField::Freshness => {
                    row(
                        frame,
                        rows[row_idx],
                        "Answer freshness",
                        s.freshness.label(),
                        focus == SettingsField::Freshness,
                        false,
                    );
                }
                SettingsField::Provider => {
                    row(
                        frame,
                        rows[row_idx],
                        "AI provider",
                        PROVIDERS[s.provider_idx].name,
                        focus == SettingsField::Provider,
                        false,
                    );
                }
                SettingsField::ProviderModel => {
                    row(
                        frame,
                        rows[row_idx],
                        "Provider model",
                        &s.provider_model,
                        focus == SettingsField::ProviderModel,
                        false,
                    );
                }
                SettingsField::ApiKey => {
                    if editing && focus == SettingsField::ApiKey {
                        key_edit_row(frame, rows[row_idx], "API key", app);
                    } else {
                        let display = if s.api_key.is_empty() {
                            "(not set)".to_string()
                        } else {
                            mask(&s.api_key)
                        };
                        row(
                            frame,
                            rows[row_idx],
                            "API key",
                            &display,
                            focus == SettingsField::ApiKey,
                            false,
                        );
                    }
                }
                SettingsField::BackupDatabase => {
                    row(
                        frame,
                        rows[row_idx],
                        "Backup database",
                        "Export snapshot…",
                        focus == SettingsField::BackupDatabase,
                        false,
                    );
                }
                SettingsField::BannedAuthors => {
                    row(
                        frame,
                        rows[row_idx],
                        "Banned publishers",
                        &format!("{} active", crate::db::banned_count()),
                        focus == SettingsField::BannedAuthors,
                        false,
                    );
                }
                SettingsField::RestoreDatabase => {
                    row(
                        frame,
                        rows[row_idx],
                        "Restore database",
                        "Import from file…",
                        focus == SettingsField::RestoreDatabase,
                        false,
                    );
                }
                #[cfg(feature = "publisher")]
                SettingsField::SyncWebArchive => {
                    row(
                        frame,
                        rows[row_idx],
                        "Sync Web Archive",
                        "Publish to mbhub.dev…",
                        focus == SettingsField::SyncWebArchive,
                        false,
                    );
                }
                SettingsField::ClearCache => {
                    row(
                        frame,
                        rows[row_idx],
                        "Clear storage cache",
                        "Purge all records…",
                        focus == SettingsField::ClearCache,
                        false,
                    );
                }
                SettingsField::TermsOfService => {
                    row(
                        frame,
                        rows[row_idx],
                        "Terms of Service",
                        "View agreement…",
                        focus == SettingsField::TermsOfService,
                        false,
                    );
                }
                // Fields hidden from non-publisher builds resolve to no-op.
                #[cfg(not(feature = "publisher"))]
                _ => {}
            },
            SettingItem::ApiEndpoint => {
                row(
                    frame,
                    rows[row_idx],
                    "API endpoint",
                    PROVIDERS[s.provider_idx].endpoint,
                    false,
                    true,
                );
            }
        }
        row_idx += 1;
    }

    let last_idx = rows.len().saturating_sub(1);
    if rows[last_idx].height >= 2 {
        draw_help_box(frame, rows[last_idx], focus, editing, with_blank_line);
    }

    // Modal Picker if active
    if let Some(picker) = &app.picker_modal {
        draw_picker_modal(frame, area, picker);
    }

    // File / Directory browser modal if active
    if let Some(browser) = &app.file_browser_modal {
        draw_file_browser_modal(frame, area, browser);
    }

    // Confirm modal if active
    if let Some(modal) = &app.confirm_modal {
        draw_confirm_modal(frame, area, &modal.title, &modal.message);
    }
}

fn draw_picker_modal(frame: &mut Frame, area: Rect, picker: &PickerModal) {
    let modal_w = 58u16.min(area.width.saturating_sub(4));
    let modal_h = 14u16.min(area.height.saturating_sub(2));

    let modal_x = area.x + (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .title(format!(" {} ({} items) ", picker.title, picker.items.len()))
        .title_alignment(Alignment::Center);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let list_h = (inner.height.saturating_sub(2)) as usize;
    if list_h == 0 {
        return;
    }

    // Scroll only when selection leaves the visible window;
    // moving within the window just moves the highlight cursor (identical to Memory list).
    let start = picker.scroll_into_view(list_h);
    let end = (start + list_h).min(picker.items.len());

    let mut lines = Vec::new();
    for i in start..end {
        let item = &picker.items[i];
        let is_selected = i == picker.selected;
        let style = if is_selected {
            Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let prefix = if is_selected { " ► " } else { "   " };
        let text = format!("{prefix}{:<width$}", item, width = (inner.width as usize).saturating_sub(4));
        lines.push(Line::from(Span::styled(text, style)));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        " ↑ / ↓ : navigate · Enter : select · Esc : cancel ",
        theme::muted(),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_help_box(
    frame: &mut Frame,
    area: Rect,
    field: SettingsField,
    editing: bool,
    with_blank_line: bool,
) {
    let width = area.width as usize;
    if width < 10 || area.height < 2 {
        return;
    }

    let (title, description, hint) = field_help(field, editing);

    let divider_prefix = "── ";
    let divider_title = title.to_string();
    let used_w = divider_prefix.len() + divider_title.len() + 1;
    let filler_w = width.saturating_sub(used_w);

    let header_line = Line::from(vec![
        Span::styled(divider_prefix, theme::muted()),
        Span::styled(divider_title, theme::accent()),
        Span::styled(format!(" {}", "─".repeat(filler_w)), theme::muted()),
    ]);

    // Word-wrapped description lines
    let desc_lines = wrap_description(description, width);

    // Word-wrapped hint lines (ensures hint never overflows narrow widths)
    let hint_formatted = format!("💡 {hint}");
    let hint_lines = wrap_description(&hint_formatted, width);

    let mut all_lines = vec![header_line];
    for d in desc_lines {
        all_lines.push(Line::from(Span::styled(d, theme::meta())));
    }
    if with_blank_line && (area.height as usize) > all_lines.len() + hint_lines.len() {
        all_lines.push(Line::raw(""));
    }
    for h in hint_lines {
        all_lines.push(Line::from(Span::styled(h, theme::muted())));
    }

    let max_lines = area.height as usize;
    let visible: Vec<Line> = all_lines.into_iter().take(max_lines).collect();
    frame.render_widget(Paragraph::new(visible), area);
}

fn field_help(
    field: SettingsField,
    editing: bool,
) -> (&'static str, &'static str, &'static str) {
    match field {
        SettingsField::DateFormat => (
            "Date format",
            "Controls timestamp presentation format across all screens (Storage, Header, Logs). Formats stay within the 16-column layout budget.",
            "Enter : open format selection modal · ← / → : cycle",
        ),
        SettingsField::Storage => (
            "Reserved storage",
            if editing {
                "Type the new storage quota in GB and press Enter to save. Default is 1 GB."
            } else {
                "Local disk budget for your node's live shard (default 1 GB). Every verified answer is stored until this quota fills up — relevance is NOT filtered at storage time. When the quota overflows, Query locality keeps the records most similar to your past questions and evicts the least relevant; Blind swarm evicts the oldest."
            },
            if editing { "Enter: save quota · Backspace: edit" } else { "Enter: edit storage quota" },
        ),
        SettingsField::ShardingMode => (
            "Sharding mode",
            "Shapes how your shard lives with the network. 'Query locality' continuously re-orders SQLite so the records most similar to your past questions stay on top and survive eviction. 'Blind swarm' stores random parts of the network with zero query tracking for maximum privacy.",
            "Enter : open sharding mode modal · ← / → : cycle",
        ),
        SettingsField::HitRate => (
            "Hit rate threshold",
            "Direct-display threshold for the ASK screen: a cached local/network answer is shown immediately only if it is at least this similar to your question; otherwise the live model is woken up. It does NOT gate storage — unrelated verified answers are still stored in the shard.",
            "Enter : open hit rate selection modal · ← / → : cycle",
        ),
        SettingsField::Freshness => (
            "Answer freshness",
            "Time window threshold for query matching. Answers older than this window are ignored during local and network lookups, prompting fresh model inference.",
            "Enter : open freshness selection modal · ← / → : cycle",
        ),
        SettingsField::Provider => (
            "AI provider",
            "Active cloud AI provider for live inference. Enter a valid API key below to automatically discover and unlock the live model catalog.",
            "Enter : open provider selection modal · ← / → : cycle",
        ),
        SettingsField::ProviderModel => (
            "Provider model",
            "Active text model fetched live from your cloud AI provider. Only models verified for pure text/chat output are listed.",
            "Enter : open model selection modal · ← / → : cycle",
        ),
        SettingsField::ApiKey => (
            "API key",
            if editing {
                "Enter your secret API key. Once saved, MBHub validates the key against the provider's endpoint and loads available models."
            } else {
                "Secret API key for the selected cloud AI provider. Stored locally on your machine and used to query live models."
            },
            if editing { "Enter: save & validate key" } else { "Enter: edit API key" },
        ),
        SettingsField::BackupDatabase => (
            "Backup database (Export)",
            "Takes a complete, portable snapshot of the SQLite database (including all inferences, SimHashes, models, and metadata) and saves it to any folder on your machine.",
            "Enter : open directory browser",
        ),
        SettingsField::RestoreDatabase => (
            "Restore database (Import)",
            "Restores a previously exported .db backup file. Your live SQLite database will be replaced with this snapshot, and all restored records will be immediately browsable in Memory.",
            "Enter : open file manager",
        ),
        SettingsField::BannedAuthors => (
            "Banned publishers",
            "Publishers you have locally banned: all of their records were removed from this node and new ones are never accepted. Bans are permanent and never propagate to other users — select a publisher below to lift the ban (already-deleted records are not restored).",
            "Enter : manage banned publishers",
        ),
        #[cfg(feature = "publisher")]
        SettingsField::SyncWebArchive => (
            "Sync Web Archive (mbhub.dev)",
            "Initiates background synchronization between local verified candidates and the public collective memory website at https://mbhub.dev. Protects indexed URLs from deletion.",
            "Enter : trigger immediate sync & deploy",
        ),
        SettingsField::ClearCache => (
            "Clear storage cache",
            "Purges all locally stored inference records and resets the local database. The database will remain completely empty until new queries are executed.",
            "Enter : open confirmation modal",
        ),
        SettingsField::TermsOfService => (
            "Terms of Service & Legal Framework",
            "Review the complete 17-section decentralized P2P operational agreement, safety boundaries, BYOK terms, and open-source licensing framework.",
            "Enter : open agreement in viewer",
        ),
        // Fields hidden from non-publisher builds resolve to no-op.
        #[cfg(not(feature = "publisher"))]
        _ => ("", "", ""),
    }
}

fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = Vec::new();
    let mut cur = String::new();

    for w in words {
        let w_len = w.chars().count();
        if w_len > width {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let chars: Vec<char> = w.chars().collect();
            let mut chunks = chars.chunks(width).peekable();
            while let Some(chunk) = chunks.next() {
                if chunks.peek().is_some() {
                    out.push(chunk.iter().collect());
                } else {
                    cur = chunk.iter().collect();
                }
            }
            continue;
        }

        if cur.is_empty() {
            cur.push_str(w);
        } else if cur.chars().count() + 1 + w_len <= width {
            cur.push(' ');
            cur.push_str(w);
        } else {
            out.push(cur);
            cur = w.to_string();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A normal (non-editing) field: label column + plain value.
fn row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: &str,
    focused: bool,
    readonly: bool,
) {
    let (label_area, value_area) = split(area);

    let label_style = if focused { theme::focus() } else { theme::muted() };
    let value_style = if focused {
        theme::focus()
    } else if readonly {
        theme::muted()
    } else {
        theme::accent()
    };

    let label_line = Line::from(Span::styled(
        format!("{:<width$}", label, width = LABEL_W as usize),
        label_style,
    ));
    frame.render_widget(Paragraph::new(label_line), label_area);

    let value_line = Line::from(Span::styled(value.to_string(), value_style));
    frame.render_widget(Paragraph::new(value_line), value_area);
}

/// Editing view for Reserved Storage: live textarea + non-editable "GB".
fn storage_edit_row(frame: &mut Frame, area: Rect, app: &App) {
    let (label_area, value_area) = split(area);

    let label_line = Line::from(Span::styled(
        format!("{:<width$}", "Reserved storage", width = LABEL_W as usize),
        theme::focus(),
    ));
    frame.render_widget(Paragraph::new(label_line), label_area);

    let suffix = " GB";
    let suffix_w = suffix.chars().count() as u16;
    let ta_w = value_area.width.saturating_sub(suffix_w);
    let ta_area = Rect::new(value_area.x, value_area.y, ta_w, 1);
    let suf_area = Rect::new(value_area.x + ta_w, value_area.y, suffix_w, 1);

    frame.render_widget(&app.edit_buffer, ta_area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(suffix, theme::accent()))),
        suf_area,
    );
}

/// Generic editing view for text fields: live textarea spanning the value column.
fn key_edit_row(frame: &mut Frame, area: Rect, label: &str, app: &App) {
    let (label_area, value_area) = split(area);

    let label_line = Line::from(Span::styled(
        format!("{:<width$}", label, width = LABEL_W as usize),
        theme::focus(),
    ));
    frame.render_widget(Paragraph::new(label_line), label_area);

    frame.render_widget(&app.edit_buffer, value_area);
}

fn split(area: Rect) -> (Rect, Rect) {
    let lw = if area.width < 35 {
        (area.width / 2).max(1)
    } else {
        LABEL_W.min(area.width)
    };
    let label = Rect::new(area.x, area.y, lw, 1);
    let value = Rect::new(area.x + lw, area.y, area.width.saturating_sub(lw), 1);
    (label, value)
}

/// Hide the key with bullets, capped so a long key cannot blow up the line.
fn mask(key: &str) -> String {
    let n = key.chars().count().min(24);
    "•".repeat(n.max(1))
}

fn draw_confirm_modal(frame: &mut Frame, area: Rect, title: &str, message: &str) {
    let modal_w = 64u16.min(area.width.saturating_sub(4));
    let modal_h = 8u16.min(area.height.saturating_sub(2));

    let modal_x = area.x + (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        .title(format!(" {title} "))
        .title_alignment(Alignment::Center);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let msg_lines = wrap_description(message, inner.width as usize);
    let mut lines = Vec::new();
    for l in msg_lines {
        lines.push(Line::from(Span::styled(l, Style::default().fg(Color::White))));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter / Y : Confirm purge ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(" Esc / N : Cancel ", Style::default().bg(Color::DarkGray).fg(Color::White)),
    ]));

    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", (bytes as f64) / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", (bytes as f64) / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", (bytes as f64) / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn draw_file_browser_modal(
    frame: &mut Frame,
    area: Rect,
    browser: &crate::app::FileBrowserModal,
) {
    let modal_w = 68u16.min(area.width.saturating_sub(4));
    let modal_h = 16u16.min(area.height.saturating_sub(2));

    let modal_x = area.x + (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .title(format!(" {} ({} entries) ", browser.title, browser.entries.len()))
        .title_alignment(Alignment::Center);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let list_h = (inner.height.saturating_sub(3)) as usize;
    if list_h == 0 {
        return;
    }

    // Top two lines: current directory path + separator.
    let path_str = browser.current_dir.display().to_string();
    let max_path_w = (inner.width as usize).saturating_sub(10);
    let trimmed_path = if path_str.len() > max_path_w {
        format!("…{}", &path_str[path_str.len().saturating_sub(max_path_w)..])
    } else {
        path_str
    };

    let header_lines = vec![
        Line::from(vec![
            Span::styled(" Path: ", theme::muted()),
            Span::styled(trimmed_path, theme::accent()),
        ]),
        Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            theme::muted(),
        )),
    ];
    frame.render_widget(
        Paragraph::new(header_lines),
        Rect::new(inner.x, inner.y, inner.width, 2),
    );

    // Scrolling list using edge-only scroll_into_view.
    let start = browser.scroll_into_view(list_h);
    let end = (start + list_h).min(browser.entries.len());

    // File size lives in a FIXED right-side column so hover/selection styling
    // can never shift its position.
    let size_cols: u16 = 10;
    let left_w = inner.width.saturating_sub(size_cols);

    for i in start..end {
        let entry = &browser.entries[i];
        let is_selected = i == browser.selected;
        let row_y = inner.y + 2 + (i - start) as u16;
        let prefix = if is_selected { " ► " } else { "   " };

        if entry.is_action {
            let style = if is_selected {
                Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            };
            let text = fit_cols(&format!("{prefix}{}", entry.name), inner.width as usize);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))),
                Rect::new(inner.x, row_y, inner.width, 1),
            );
        } else if entry.is_dir {
            let style = if is_selected {
                Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            };
            let text = fit_cols(&format!("{prefix}📁 {}", entry.name), inner.width as usize);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))),
                Rect::new(inner.x, row_y, inner.width, 1),
            );
        } else {
            let is_db = entry.name.ends_with(".db") || entry.name.ends_with(".sqlite");
            let style = if is_selected {
                Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
            } else if is_db {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Left column: prefix + icon + name, clipped to its fixed area.
            let name_text = fit_cols(
                &format!("{prefix}📄 {}", entry.name),
                left_w as usize,
            );
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(name_text, style))),
                Rect::new(inner.x, row_y, left_w, 1),
            );

            // Right column: size, right-aligned, fixed position regardless of
            // hover state or name length.
            let size = format_size(entry.size_bytes);
            let size_text = format!("{:>width$}", size, width = size_cols as usize);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(size_text, style))),
                Rect::new(inner.x + left_w, row_y, size_cols, 1),
            );
        }
    }

    // Help bar at bottom.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Enter : open/select · Backspace : parent · Esc : cancel ",
            theme::muted(),
        ))),
        Rect::new(inner.x, inner.y + 2 + list_h as u16, inner.width, 1),
    );
}

/// Truncates (with ellipsis) or pads `s` to exactly `cols` display columns,
/// accounting for wide glyphs like emoji.
fn fit_cols(s: &str, cols: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    let current = UnicodeWidthStr::width(s);
    if current <= cols {
        let mut out = s.to_string();
        while UnicodeWidthStr::width(out.as_str()) < cols {
            out.push(' ');
        }
        return out;
    }

    if cols == 0 {
        return String::new();
    }
    if cols == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if width + w + 1 > cols {
            break; // leave room for the ellipsis
        }
        width += w;
        out.push(c);
    }
    while width + 1 < cols {
        out.push(' ');
        width += 1;
    }
    out.push('…');
    out
}
