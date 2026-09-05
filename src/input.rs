//! A tiny soft-wrapping text input for the search box.
//!
//! tui-textarea does not wrap (it scrolls horizontally), and the Ask screen
//! needs text to flow onto the next visual line at the right edge. This is a
//! minimal editor over a flat `Vec<char>`: hard newlines are kept, soft wraps
//! are computed on demand from the render width.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

pub const MAX_CHARS: usize = 80;

#[derive(Clone, Debug)]
pub struct QueryInput {
    chars: Vec<char>,
    /// Cursor as a char index into `chars`, always on a boundary (0..=len).
    cursor: usize,
}

impl QueryInput {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
        }
    }

    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent, width: usize) {
        let ctrl_alt = key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if !ctrl_alt => self.insert_char(c),
            KeyCode::Enter => self.insert_char('\n'),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_up(width),
            KeyCode::Down => self.move_down(width),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            _ => {}
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if self.chars.len() >= MAX_CHARS {
            return;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Empties the input completely (cursor returns to position 0).
    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if self.chars.len() >= MAX_CHARS {
                break;
            }
            self.insert_char(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.chars.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        let mut i = self.cursor;
        while i > 0 && self.chars[i - 1] != '\n' {
            i -= 1;
        }
        self.cursor = i;
    }

    pub fn move_end(&mut self) {
        let mut i = self.cursor;
        while i < self.chars.len() && self.chars[i] != '\n' {
            i += 1;
        }
        self.cursor = i;
    }

    pub fn move_up(&mut self, width: usize) {
        let (line, col) = self.cursor_line_col(width);
        if line > 0 {
            self.cursor = index_at_col(&self.chars, width, line - 1, col);
        } else {
            self.cursor = 0;
        }
    }

    pub fn move_down(&mut self, width: usize) {
        let (line, col) = self.cursor_line_col(width);
        let n = visual_lines(&self.chars, width).len();
        if line + 1 < n {
            self.cursor = index_at_col(&self.chars, width, line + 1, col);
        } else {
            self.cursor = self.chars.len();
        }
    }

    pub fn visual_line_count(&self, width: usize) -> usize {
        visual_lines(&self.chars, width).len()
    }

    /// Each visual line rendered as a `String`, for the drawing code.
    pub fn visual_text(&self, width: usize) -> Vec<String> {
        visual_lines(&self.chars, width)
            .into_iter()
            .map(|(s, e)| self.chars[s..e].iter().collect())
            .collect()
    }

    /// Visual (line index, column) of the cursor for a given render width.
    pub fn cursor_line_col(&self, width: usize) -> (usize, usize) {
        cursor_line_col(&self.chars, width, self.cursor)
    }

    /// The glyph rendered under the cursor (a space when past the end / at a
    /// hard newline).
    pub fn char_under_cursor(&self) -> String {
        if self.cursor < self.chars.len() && self.chars[self.cursor] != '\n' {
            self.chars[self.cursor].to_string()
        } else {
            " ".to_string()
        }
    }
}

impl Default for QueryInput {
    fn default() -> Self {
        Self::new()
    }
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Split `chars` into visual lines `(start, end)` for `width` columns. Hard
/// newlines terminate a line; otherwise a line soft-wraps before the char that
/// would overflow.
pub fn visual_lines(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut col = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            lines.push((start, i));
            i += 1;
            start = i;
            col = 0;
        } else {
            let w = char_width(c);
            if col > 0 && col + w > width {
                lines.push((start, i));
                start = i;
                col = 0;
            } else {
                col += w;
                i += 1;
            }
        }
    }
    lines.push((start, chars.len()));
    lines
}

/// Resolve the visual line and column of `cursor` at `width`.
pub fn cursor_line_col(chars: &[char], width: usize, cursor: usize) -> (usize, usize) {
    let lines = visual_lines(chars, width);
    let idx = if cursor == chars.len() {
        lines.len().saturating_sub(1)
    } else if chars[cursor] == '\n' {
        // Cursor sits at the end of the visual line terminated by this newline.
        lines
            .iter()
            .position(|&(_, e)| e == cursor)
            .unwrap_or_else(|| {
                lines
                    .iter()
                    .position(|&(s, e)| s <= cursor && cursor < e)
                    .unwrap_or(0)
            })
    } else {
        lines
            .iter()
            .position(|&(s, e)| s <= cursor && cursor < e)
            .unwrap_or(0)
    };

    let (start, _) = lines[idx];
    let col = chars[start..cursor]
        .iter()
        .filter(|c| **c != '\n')
        .map(|c| char_width(*c))
        .sum::<usize>();
    (idx, col)
}

/// Map a visual `col` on `line_idx` back to a char index (clamped to line end).
fn index_at_col(chars: &[char], width: usize, line_idx: usize, col: usize) -> usize {
    let lines = visual_lines(chars, width);
    let (start, end) = lines[line_idx];
    let mut acc = 0usize;
    let mut i = start;
    while i < end {
        let w = char_width(chars[i]);
        if acc + w > col {
            break;
        }
        acc += w;
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(s: &str) -> QueryInput {
        let mut q = QueryInput::new();
        q.insert_str(s);
        q
    }

    #[test]
    fn wraps_at_width() {
        let chars: Vec<char> = "abcdef".chars().collect();
        assert_eq!(visual_lines(&chars, 3), vec![(0, 3), (3, 6)]);
        assert_eq!(inp("abcdef").visual_line_count(3), 2);
    }

    #[test]
    fn hard_newline_splits() {
        let chars: Vec<char> = "abc\ndef".chars().collect();
        assert_eq!(visual_lines(&chars, 10), vec![(0, 3), (4, 7)]);
    }

    #[test]
    fn empty_is_one_line() {
        let q = QueryInput::new();
        assert_eq!(q.visual_line_count(40), 1);
    }

    #[test]
    fn caps_at_80() {
        let mut q = QueryInput::new();
        q.insert_str(&"a".repeat(500));
        assert_eq!(q.char_count(), 80);
    }

    #[test]
    fn cursor_up_down_preserves_column() {
        // 3 cols per visual line: "abc" / "def"
        let mut q = inp("abcdef");
        q.move_end(); // cursor = 6 (end)
        q.move_up(3); // line 0, col 3
        assert_eq!(q.cursor, 3);
        q.move_down(3); // back to line 1, col 3
        assert_eq!(q.cursor, 6);
    }
}
