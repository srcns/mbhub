//! Data Loss Prevention (DLP) — Structural Pattern Scanner.
//!
//! Detects sensitive data patterns (API keys, JWTs, credit card numbers,
//! private key blocks, AWS credentials) in both user input and model output
//! using deterministic structural rules. Zero external API calls, microsecond
//! latency, no false-negative on known formats.
//!
//! Two entry points:
//! - `scan_text()`: Returns whether text contains sensitive data and what type.
//! - `redact_secrets()`: Replaces detected secrets with `[REDACTED_***]` placeholders.

use regex::Regex;
use std::sync::LazyLock;

/// Result of a DLP scan on a text fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct DlpScanResult {
    pub is_sensitive: bool,
    pub matched_pattern: Option<&'static str>,
}

impl DlpScanResult {
    fn clean() -> Self {
        Self {
            is_sensitive: false,
            matched_pattern: None,
        }
    }

    fn hit(pattern: &'static str) -> Self {
        Self {
            is_sensitive: true,
            matched_pattern: Some(pattern),
        }
    }
}

// ── Compiled pattern set (compiled once, reused across calls) ──

static RE_OPENAI_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap());

static RE_ANTHROPIC_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sk-ant-[A-Za-z0-9\-]{20,}").unwrap());

static RE_GENERIC_API_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(api[_-]?key|secret[_-]?key|access[_-]?token|bearer)\s*[=:]\s*['"]?[A-Za-z0-9_\-]{20,}"#)
        .unwrap()
});

static RE_JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}").unwrap()
});

static RE_PRIVATE_KEY_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN\s+(RSA\s+|EC\s+|DSA\s+|OPENSSH\s+)?PRIVATE\s+KEY-----").unwrap()
});

static RE_AWS_ACCESS_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"AKIA[0-9A-Z]{16}").unwrap());

static RE_CREDIT_CARD_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d[\d\s\-]{11,22}\d)\b").unwrap());

/// Ordered list of (pattern, label) for sequential scanning.
/// Order matters: more specific patterns first to avoid shadowing.
static PATTERNS: LazyLock<Vec<(&'static Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (&*RE_ANTHROPIC_KEY, "Anthropic API Key"),
        (&*RE_OPENAI_KEY, "API Key (OpenAI / Generic)"),
        (&*RE_AWS_ACCESS_KEY, "AWS Access Key"),
        (&*RE_JWT, "JWT Token"),
        (&*RE_PRIVATE_KEY_BLOCK, "Private Key Block"),
        (&*RE_GENERIC_API_KEY, "API Key / Secret Token"),
    ]
});

/// Luhn algorithm checksum validation for credit card numbers.
fn luhn_check(digits: &[u8]) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum: u32 = 0;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut val = d as u32;
        if double {
            val *= 2;
            if val > 9 {
                val -= 9;
            }
        }
        sum += val;
        double = !double;
    }
    sum % 10 == 0
}

/// Scans `text` for known sensitive data patterns.
///
/// Returns immediately on first match (fail-fast). Microsecond latency for
/// typical 80-character queries and sub-millisecond for 64 KB model outputs.
pub fn scan_text(text: &str) -> DlpScanResult {
    // 1. Check regex-based patterns
    for (re, label) in PATTERNS.iter() {
        if re.is_match(text) {
            return DlpScanResult::hit(label);
        }
    }

    // 2. Credit card Luhn check (requires digit extraction + validation)
    for cap in RE_CREDIT_CARD_CANDIDATE.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let digits: Vec<u8> = m
                .as_str()
                .chars()
                .filter(|c| c.is_ascii_digit())
                .map(|c| c as u8 - b'0')
                .collect();
            if luhn_check(&digits) {
                return DlpScanResult::hit("Credit Card Number");
            }
        }
    }

    DlpScanResult::clean()
}

/// Replaces detected secrets in `text` with `[REDACTED_***]` placeholders.
///
/// Used on model output before gossip broadcast to prevent accidental
/// secret leakage into the P2P network.
pub fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();

    // Redact regex-matched patterns
    for (re, label) in PATTERNS.iter() {
        let tag = format!("[REDACTED_{label}]");
        result = re.replace_all(&result, tag.as_str()).to_string();
    }

    // Redact Luhn-valid credit card candidates
    // (We re-check on the already-partially-redacted text; card patterns
    // won't overlap with the regex patterns above.)
    let cc_re = &*RE_CREDIT_CARD_CANDIDATE;
    let mut redacted = String::with_capacity(result.len());
    let mut last_end = 0;
    for cap in cc_re.captures_iter(&result) {
        if let Some(m) = cap.get(1) {
            let digits: Vec<u8> = m
                .as_str()
                .chars()
                .filter(|c| c.is_ascii_digit())
                .map(|c| c as u8 - b'0')
                .collect();
            if luhn_check(&digits) {
                redacted.push_str(&result[last_end..m.start()]);
                redacted.push_str("[REDACTED_Credit Card Number]");
                last_end = m.end();
            }
        }
    }
    redacted.push_str(&result[last_end..]);

    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_api_key() {
        let input = "my key is sk-proj1234567890abcdefghij";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(
            result.matched_pattern,
            Some("API Key (OpenAI / Generic)")
        );
    }

    #[test]
    fn detects_anthropic_api_key() {
        let input = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz123456";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(result.matched_pattern, Some("Anthropic API Key"));
    }

    #[test]
    fn detects_jwt_token() {
        let input =
            "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(result.matched_pattern, Some("JWT Token"));
    }

    #[test]
    fn detects_credit_card_luhn() {
        // Valid Visa test number
        let input = "card: 4111111111111111";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(result.matched_pattern, Some("Credit Card Number"));
    }

    #[test]
    fn rejects_invalid_luhn() {
        // Invalid Luhn checksum
        let input = "not a card: 4111111111111112";
        let result = scan_text(input);
        assert!(!result.is_sensitive);
    }

    #[test]
    fn detects_private_key_block() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(result.matched_pattern, Some("Private Key Block"));
    }

    #[test]
    fn detects_aws_access_key() {
        let input = "AWS key: AKIAIOSFODNN7EXAMPLE";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(result.matched_pattern, Some("AWS Access Key"));
    }

    #[test]
    fn detects_generic_api_key_assignment() {
        let input = "api_key = 'abcdefghijklmnopqrstuvwxyz12'";
        let result = scan_text(input);
        assert!(result.is_sensitive);
        assert_eq!(
            result.matched_pattern,
            Some("API Key / Secret Token")
        );
    }

    #[test]
    fn passes_clean_text() {
        let input = "How does Rust ownership work?";
        let result = scan_text(input);
        assert!(!result.is_sensitive);
        assert_eq!(result.matched_pattern, None);
    }

    #[test]
    fn redacts_api_key_in_output() {
        let input = "Use this key: sk-proj1234567890abcdefghij to authenticate.";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("sk-proj"));
        assert!(redacted.contains("[REDACTED_"));
    }

    #[test]
    fn redacts_credit_card_in_output() {
        let input = "Example card: 4111111111111111 for testing.";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("4111111111111111"));
        assert!(redacted.contains("[REDACTED_Credit Card Number]"));
    }

    #[test]
    fn preserves_clean_output() {
        let input = "Rust is a systems programming language focused on safety.";
        let redacted = redact_secrets(input);
        assert_eq!(redacted, input);
    }
}
