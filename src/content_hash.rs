//! Cryptographic Content-Addressing via BLAKE3.
//!
//! Every inference record is identified by a deterministic 256-bit content hash:
//!   `content_hash = BLAKE3(question || "\x00" || content || "\x00" || provider || "\x00" || model)`
//!
//! This provides:
//! - **Tamper detection:** Any modification to any field invalidates the hash.
//! - **Content-based identity:** Records with identical content produce identical hashes
//!   regardless of creation time or SQLite row ID.
//! - **Non-repudiation foundation:** Combined with Ed25519 signatures (Phase 2),
//!   this hash becomes the signing payload.

/// Computes a BLAKE3 content hash over the canonical fields of an inference record.
///
/// Fields are separated by null bytes (`\x00`) to prevent ambiguity between
/// field boundaries (e.g., `("ab", "cd")` vs `("a", "bcd")`).
pub fn compute_content_hash(
    question: &str,
    content: &str,
    provider: &str,
    model: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(question.as_bytes());
    hasher.update(b"\x00");
    hasher.update(content.as_bytes());
    hasher.update(b"\x00");
    hasher.update(provider.as_bytes());
    hasher.update(b"\x00");
    hasher.update(model.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Verifies that a stored content hash matches the record's actual fields.
///
/// Returns `true` if the hash is consistent, `false` if the record has been
/// tampered with or corrupted.
#[allow(dead_code)]
pub fn verify_content_hash(
    stored_hash: &str,
    question: &str,
    content: &str,
    provider: &str,
    model: &str,
) -> bool {
    if stored_hash.is_empty() {
        // Legacy records without content_hash — cannot verify, treat as unverified.
        return false;
    }
    let computed = compute_content_hash(question, content, provider, model);
    computed == stored_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_hash() {
        let h1 = compute_content_hash("What is Rust?", "Systems language.", "OpenAI", "gpt-4o");
        let h2 = compute_content_hash("What is Rust?", "Systems language.", "OpenAI", "gpt-4o");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // 256-bit hex
    }

    #[test]
    fn different_content_different_hash() {
        let h1 = compute_content_hash("What is Rust?", "Systems language.", "OpenAI", "gpt-4o");
        let h2 = compute_content_hash("What is Rust?", "A fast language.", "OpenAI", "gpt-4o");
        assert_ne!(h1, h2);
    }

    #[test]
    fn different_provider_different_hash() {
        let h1 = compute_content_hash("What is Rust?", "Answer.", "OpenAI", "gpt-4o");
        let h2 = compute_content_hash("What is Rust?", "Answer.", "Anthropic", "claude-3");
        assert_ne!(h1, h2);
    }

    #[test]
    fn verify_roundtrip() {
        let hash =
            compute_content_hash("Question?", "Answer.", "Gemini", "gemini-2.5-pro");
        assert!(verify_content_hash(
            &hash,
            "Question?",
            "Answer.",
            "Gemini",
            "gemini-2.5-pro"
        ));
    }

    #[test]
    fn verify_detects_tampering() {
        let hash =
            compute_content_hash("Question?", "Answer.", "Gemini", "gemini-2.5-pro");
        // Tamper with content
        assert!(!verify_content_hash(
            &hash,
            "Question?",
            "TAMPERED.",
            "Gemini",
            "gemini-2.5-pro"
        ));
    }

    #[test]
    fn verify_empty_hash_returns_false() {
        assert!(!verify_content_hash(
            "",
            "Question?",
            "Answer.",
            "OpenAI",
            "gpt-4o"
        ));
    }

    #[test]
    fn field_boundary_ambiguity_prevented() {
        // These two inputs should produce different hashes due to null-byte separators
        let h1 = compute_content_hash("ab", "cd", "e", "f");
        let h2 = compute_content_hash("a", "bcd", "e", "f");
        assert_ne!(h1, h2);
    }
}
