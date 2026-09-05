//! Fast, terminal-friendly markdown parser and word-wrapper.
//!
//! Produces a `Vec<Line<'static>>` sized for a given column width.
//! Supports:
//! - Headings (`#`, `##`, `###`)
//! - Bold (`**text**`, `__text__`)
//! - Italic (`*text*`, `_text_`)
//! - Inline code (`` `code` ``)
//! - Code blocks (` ```lang ` ... ` ``` `)
//! - Bullet lists (`- `, `* `) and numbered lists (`1. `)
//! - Blockquotes (`> `)
//! - Horizontal rules (`---`, `***`)
//! - Word-boundary wrapping to prevent mid-word splits

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme;

/// Heading styles
fn h1_style() -> Style {
    Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

fn h2_style() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

fn h3_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn code_block_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn inline_code_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

fn bold_style() -> Style {
    Style::default()
        .fg(theme::TEXT)
        .add_modifier(Modifier::BOLD)
}

fn italic_style() -> Style {
    Style::default()
        .fg(theme::TEXT)
        .add_modifier(Modifier::ITALIC)
}

fn quote_bar_style() -> Style {
    Style::default().fg(theme::ACCENT)
}

fn quote_text_style() -> Style {
    Style::default().fg(theme::META)
}

fn bullet_style() -> Style {
    Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn hr_style() -> Style {
    Style::default().fg(theme::MUTED)
}

/// Convert a markdown string into styled visual lines wrapped at `width`.
///
/// Security: All input is sanitized through `strip_control_chars` before
/// rendering to prevent ANSI escape injection (OSC 52 clipboard hijacking,
/// CSI cursor manipulation, etc.) from untrusted P2P network content.
pub fn render_markdown(raw: &str, width: usize) -> Vec<Line<'static>> {
    let sanitized = crate::sanitize::strip_control_chars(raw);
    let width = width.max(10);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;

    let lines: Vec<&str> = sanitized.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Check for fenced code block toggle
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            i += 1;
            continue;
        }

        if in_code_block {
            // In code blocks, preserve indentation, render with code style
            let code_line = wrap_code_line(line, width);
            out.extend(code_line);
            i += 1;
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let hr = "─".repeat(width);
            out.push(Line::from(Span::styled(hr, hr_style())));
            i += 1;
            continue;
        }

        // Headings
        if let Some(rest) = line.strip_prefix("# ") {
            let wrapped = wrap_styled_text(rest.trim(), width, h1_style(), "");
            out.extend(wrapped);
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            let wrapped = wrap_styled_text(rest.trim(), width, h2_style(), "");
            out.extend(wrapped);
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("### ") {
            let wrapped = wrap_styled_text(rest.trim(), width, h3_style(), "");
            out.extend(wrapped);
            i += 1;
            continue;
        }

        // Blockquote
        if let Some(rest) = line.strip_prefix("> ") {
            let wrapped = wrap_blockquote(rest.trim(), width);
            out.extend(wrapped);
            i += 1;
            continue;
        }

        // Unordered list item
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            let wrapped = wrap_list_item("• ", rest.trim(), width);
            out.extend(wrapped);
            i += 1;
            continue;
        }

        // Ordered list item (e.g. "1. ", "23. ")
        if let Some((prefix, rest)) = parse_numbered_list(line) {
            let wrapped = wrap_list_item(&prefix, rest.trim(), width);
            out.extend(wrapped);
            i += 1;
            continue;
        }

        // Empty line
        if trimmed.is_empty() {
            out.push(Line::raw(""));
            i += 1;
            continue;
        }

        // Normal paragraph text with inline formatting
        let wrapped = wrap_paragraph(line, width);
        out.extend(wrapped);
        i += 1;
    }

    out
}

/// Detects "1. ", "2. ", etc. and returns (prefix, rest)
fn parse_numbered_list(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    let num_digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if num_digits > 0 {
        let after_digits = &trimmed[num_digits..];
        if let Some(rest) = after_digits.strip_prefix(". ") {
            let prefix = format!("{}. ", &trimmed[..num_digits]);
            return Some((prefix, rest));
        }
    }
    None
}

/// Wrap a verbatim code line. If it exceeds width, it wraps without breaking indentation.
fn wrap_code_line(line: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let prefix = "  ";
    let max_w = width.saturating_sub(prefix.len()).max(1);

    if line.is_empty() {
        out.push(Line::raw(""));
        return out;
    }

    let mut remaining = line;
    while !remaining.is_empty() {
        let chunk: String = remaining.chars().take(max_w).collect();
        let chunk_len = chunk.len();
        out.push(Line::from(vec![
            Span::styled(prefix, quote_bar_style()),
            Span::styled(chunk, code_block_style()),
        ]));
        remaining = &remaining[chunk_len..];
    }
    out
}

/// Wraps blockquote with a leading vertical bar
fn wrap_blockquote(text: &str, width: usize) -> Vec<Line<'static>> {
    let bar = "│ ";
    let bar_w = bar.width();
    let content_w = width.saturating_sub(bar_w).max(1);

    let parsed_spans = parse_inline(text, quote_text_style());
    let wrapped_lines = wrap_spans(parsed_spans, content_w);

    wrapped_lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::styled(bar, quote_bar_style())];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// Wraps a list item with a hanging indent
fn wrap_list_item(prefix_str: &str, text: &str, width: usize) -> Vec<Line<'static>> {
    let prefix_w = prefix_str.width();
    let indent_spaces = " ".repeat(prefix_w);
    let content_w = width.saturating_sub(prefix_w).max(1);

    let parsed_spans = parse_inline(text, theme::text());
    let wrapped_lines = wrap_spans(parsed_spans, content_w);

    let mut out = Vec::new();
    for (idx, line) in wrapped_lines.into_iter().enumerate() {
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(prefix_str.to_string(), bullet_style()));
        } else {
            spans.push(Span::raw(indent_spaces.clone()));
        }
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }

    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(
            prefix_str.to_string(),
            bullet_style(),
        )]));
    }
    out
}

/// Wraps uniform styled text (such as headings)
fn wrap_styled_text(
    text: &str,
    width: usize,
    style: Style,
    _indent: &str,
) -> Vec<Line<'static>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![Line::from(Span::styled(text.to_string(), style))];
    }

    let mut out = Vec::new();
    let mut cur_line = String::new();
    let mut cur_w = 0;

    for word in words {
        let word_w = word.width();
        if word_w > width {
            if !cur_line.is_empty() {
                out.push(Line::from(Span::styled(std::mem::take(&mut cur_line), style)));
                cur_w = 0;
            }
            let mut rem = word;
            while !rem.is_empty() {
                let sub: String = rem.chars().take(width).collect();
                let sub_w = sub.width();
                let sub_len = sub.len();
                rem = &rem[sub_len..];
                if rem.is_empty() {
                    cur_line = sub;
                    cur_w = sub_w;
                } else {
                    out.push(Line::from(Span::styled(sub, style)));
                }
            }
            continue;
        }

        if cur_w == 0 {
            cur_line.push_str(word);
            cur_w += word_w;
        } else if cur_w + 1 + word_w <= width {
            cur_line.push(' ');
            cur_line.push_str(word);
            cur_w += 1 + word_w;
        } else {
            out.push(Line::from(Span::styled(cur_line, style)));
            cur_line = word.to_string();
            cur_w = word_w;
        }
    }

    if !cur_line.is_empty() {
        out.push(Line::from(Span::styled(cur_line, style)));
    }

    out
}

/// Wraps a normal paragraph with full inline formatting support
fn wrap_paragraph(text: &str, width: usize) -> Vec<Line<'static>> {
    let parsed_spans = parse_inline(text, theme::text());
    wrap_spans(parsed_spans, width)
}

/// An inline atom: a string slice with its specific Style
#[derive(Clone, Debug)]
struct StyledAtom {
    text: String,
    style: Style,
}

/// Parse inline markdown (bold, italic, code) into a sequence of StyledAtoms
fn parse_inline(input: &str, default_style: Style) -> Vec<StyledAtom> {
    let mut atoms = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut cur = String::new();

    while i < len {
        // Inline code: `...`
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let code_end = i + 1 + end;
                if !cur.is_empty() {
                    atoms.push(StyledAtom {
                        text: std::mem::take(&mut cur),
                        style: default_style,
                    });
                }
                let code_content: String = chars[i + 1..code_end].iter().collect();
                atoms.push(StyledAtom {
                    text: format!("`{code_content}`"),
                    style: inline_code_style(),
                });
                i = code_end + 1;
                continue;
            }
        }

        // Bold: **...** or __...__
        if (chars[i] == '*' && i + 1 < len && chars[i + 1] == '*')
            || (chars[i] == '_' && i + 1 < len && chars[i + 1] == '_')
        {
            let delim = chars[i];
            // Look for matching closing delim
            let mut close_idx = None;
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == delim && chars[j + 1] == delim {
                    close_idx = Some(j);
                    break;
                }
                j += 1;
            }

            if let Some(end) = close_idx {
                if !cur.is_empty() {
                    atoms.push(StyledAtom {
                        text: std::mem::take(&mut cur),
                        style: default_style,
                    });
                }
                let bold_content: String = chars[i + 2..end].iter().collect();
                atoms.push(StyledAtom {
                    text: bold_content,
                    style: bold_style(),
                });
                i = end + 2;
                continue;
            }
        }

        // Italic: *...* or _..._
        if chars[i] == '*' || chars[i] == '_' {
            let delim = chars[i];
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == delim) {
                let italic_end = i + 1 + end;
                if !cur.is_empty() {
                    atoms.push(StyledAtom {
                        text: std::mem::take(&mut cur),
                        style: default_style,
                    });
                }
                let italic_content: String = chars[i + 1..italic_end].iter().collect();
                atoms.push(StyledAtom {
                    text: italic_content,
                    style: italic_style(),
                });
                i = italic_end + 1;
                continue;
            }
        }

        cur.push(chars[i]);
        i += 1;
    }

    if !cur.is_empty() {
        atoms.push(StyledAtom {
            text: cur,
            style: default_style,
        });
    }

    atoms
}

/// Wraps a sequence of styled atoms onto visual lines of maximum `width` columns,
/// respecting word boundaries.
fn wrap_spans(atoms: Vec<StyledAtom>, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_line_width: usize = 0;

    for atom in atoms {
        // Split atom text into words and spaces
        let chunks = split_words_and_spaces(&atom.text);

        for chunk in chunks {
            let is_space = chunk.chars().all(|c| c == ' ');
            let chunk_w = chunk.width();

            // If we are at the start of a line and this is whitespace, skip it
            if current_line_width == 0 && is_space {
                continue;
            }

            if current_line_width + chunk_w <= width {
                // Fits in current line
                push_span(&mut current_spans, chunk, atom.style);
                current_line_width += chunk_w;
            } else if !is_space && chunk_w > width {
                // Word is larger than full width; break it into chunks
                if current_line_width > 0 {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    current_line_width = 0;
                }
                let mut word_rem = chunk.as_str();
                while !word_rem.is_empty() {
                    let sub_chunk: String = word_rem.chars().take(width).collect();
                    let sub_w = sub_chunk.width();
                    let sub_len = sub_chunk.len();
                    if sub_w <= width && word_rem.len() > sub_len {
                        lines.push(Line::from(vec![Span::styled(sub_chunk, atom.style)]));
                    } else {
                        push_span(&mut current_spans, sub_chunk, atom.style);
                        current_line_width = sub_w;
                    }
                    word_rem = &word_rem[sub_len..];
                }
            } else {
                // Word doesn't fit; start new line
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    current_line_width = 0;
                }
                if !is_space {
                    push_span(&mut current_spans, chunk, atom.style);
                    current_line_width = chunk_w;
                }
            }
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    if lines.is_empty() {
        lines.push(Line::raw(""));
    }

    lines
}

/// Helper to merge adjacent spans with the same style
fn push_span(spans: &mut Vec<Span<'static>>, text: String, style: Style) {
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            let mut merged = last.content.to_string();
            merged.push_str(&text);
            *last = Span::styled(merged, style);
            return;
        }
    }
    spans.push(Span::styled(text, style));
}

/// Splits text into alternating words and whitespace runs
fn split_words_and_spaces(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_is_space = None;

    for c in text.chars() {
        let is_space = c.is_whitespace();
        match current_is_space {
            None => {
                current.push(c);
                current_is_space = Some(is_space);
            }
            Some(flag) if flag == is_space => {
                current.push(c);
            }
            Some(_) => {
                result.push(std::mem::take(&mut current));
                current.push(c);
                current_is_space = Some(is_space);
            }
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_and_bold() {
        let md = "# Title\n\nThis is **bold** and `code` test.";
        let lines = render_markdown(md, 80);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn wraps_long_paragraph_at_word_boundaries() {
        let md = "One two three four five six seven eight nine ten";
        let lines = render_markdown(md, 15);
        // Should not split words
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.width() <= 15, "line was too long: '{}'", text);
        }
    }

    #[test]
    fn parses_code_blocks() {
        let md = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
        let lines = render_markdown(md, 80);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn parses_bullet_and_numbered_lists() {
        let md = "- Item A\n- Item B\n1. Numbered One\n2. Numbered Two";
        let lines = render_markdown(md, 80);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn wraps_words_wider_than_width_without_overflow() {
        let md = "# SupercalifragilisticexpialidociousHeading";
        let lines = render_markdown(md, 12);
        assert!(lines.len() > 1);
        for line in lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.width() <= 12, "Line was too long: '{}'", text);
        }
    }
}
