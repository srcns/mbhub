//! Terminal Escape Sequence Sanitizer.
//!
//! Strips all ANSI/VT100 escape sequences, OSC sequences (including OSC 52
//! clipboard hijacking), CSI cursor commands, and raw control characters from
//! untrusted text before rendering to TUI or persisting to SQLite.
//!
//! Preserves: newline (\n), carriage return (\r), tab (\t).
//! Strips: all other C0/C1 control codes, ESC-initiated sequences.
//!
//! Runs in O(n) single pass with a minimal state machine — no regex or
//! external crate required.

/// State machine for parsing escape sequences.
#[derive(Clone, Copy, PartialEq)]
enum State {
    /// Normal text passthrough.
    Normal,
    /// Seen ESC (\x1b), waiting for sequence type indicator.
    Escape,
    /// Inside CSI sequence (\x1b[...), consuming until final byte.
    Csi,
    /// Inside OSC sequence (\x1b]...), consuming until ST or BEL.
    Osc,
    /// Inside OSC, just saw ESC — next char might be '\' (ST terminator).
    OscEscaped,
}

/// Strips all terminal control characters and escape sequences from `input`.
///
/// Preserves `\n`, `\r`, and `\t` as they are legitimate text formatting.
/// Everything else in the C0 range (`\x00`..`\x1f` except the three above),
/// DEL (`\x7f`), and all ESC-initiated sequences (CSI, OSC, etc.) are removed.
pub fn strip_control_chars(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut state = State::Normal;

    for ch in input.chars() {
        match state {
            State::Normal => {
                if ch == '\x1b' {
                    state = State::Escape;
                } else if ch == '\n' || ch == '\r' || ch == '\t' {
                    output.push(ch);
                } else if ch.is_control() {
                    // Strip all other control characters (C0, DEL, C1)
                } else {
                    output.push(ch);
                }
            }
            State::Escape => match ch {
                '[' => state = State::Csi,
                ']' => state = State::Osc,
                // Single-character escape commands (e.g., ESC D, ESC M, ESC 7, ESC 8)
                // and two-character sequences (ESC (, ESC ), etc.) — consume and return.
                _ => state = State::Normal,
            },
            State::Csi => {
                // CSI sequence: \x1b[ followed by parameter/intermediate bytes (0x20-0x3F)
                // and terminated by a final byte (0x40-0x7E).
                if ('@'..='~').contains(&ch) {
                    state = State::Normal;
                }
                // Otherwise keep consuming parameter bytes
            }
            State::Osc => {
                // OSC sequence: \x1b] ... terminated by BEL (\x07) or ST (\x1b\).
                if ch == '\x07' {
                    state = State::Normal;
                } else if ch == '\x1b' {
                    state = State::OscEscaped;
                }
                // Otherwise keep consuming OSC payload
            }
            State::OscEscaped => {
                // After ESC inside OSC — if '\' follows, that's ST (String Terminator).
                state = State::Normal;
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_plain_text() {
        assert_eq!(strip_control_chars("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        let input = "line1\nline2\ttab\rcarriage";
        assert_eq!(strip_control_chars(input), input);
    }

    #[test]
    fn strips_ansi_color_codes() {
        let input = "\x1b[31mRED TEXT\x1b[0m";
        assert_eq!(strip_control_chars(input), "RED TEXT");
    }

    #[test]
    fn strips_osc52_clipboard_hijack() {
        // OSC 52 clipboard write attempt: \x1b]52;c;BASE64\x07
        let input = "safe text\x1b]52;c;bWFsaWNpb3Vz\x07 more text";
        assert_eq!(strip_control_chars(input), "safe text more text");
    }

    #[test]
    fn strips_osc_with_st_terminator() {
        // OSC terminated with ST (\x1b\)
        let input = "before\x1b]0;evil title\x1b\\after";
        assert_eq!(strip_control_chars(input), "beforeafter");
    }

    #[test]
    fn strips_csi_cursor_movement() {
        // CSI sequence to move cursor: \x1b[10;20H
        let input = "start\x1b[10;20Hend";
        assert_eq!(strip_control_chars(input), "startend");
    }

    #[test]
    fn strips_null_and_other_control_chars() {
        let input = "hello\x00world\x01test\x7f";
        assert_eq!(strip_control_chars(input), "helloworldtest");
    }

    #[test]
    fn handles_nested_and_multiple_sequences() {
        let input = "\x1b[1m\x1b[31mBOLD RED\x1b[0m \x1b]52;c;dGVzdA==\x07 normal";
        assert_eq!(strip_control_chars(input), "BOLD RED  normal");
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(strip_control_chars(""), "");
    }

    #[test]
    fn handles_unicode_content() {
        let input = "Türkçe içerik: ğüşöç 🚀 αβγ";
        assert_eq!(strip_control_chars(input), input);
    }

    #[test]
    fn strips_escape_followed_by_incomplete_sequence() {
        // Lone ESC at end of string
        let input = "text\x1b";
        assert_eq!(strip_control_chars(input), "text");
    }
}
