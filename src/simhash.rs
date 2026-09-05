//! 64-bit SimHash (Character 3-Grams) Engine.
//!
//! Provides sub-millisecond similarity matching and blind rendezvous discovery
//! for atomic queries (<= 80 characters).
//!
//! - Produces an ultra-compact 8-byte (`u64`) fingerprint.
//! - Compares candidates in 0.3 nanoseconds using CPU `POPCNT` (`(a ^ b).count_ones()`).
//! - Character 3-grams provide high recall and tolerance for typos and language suffixes.

/// Computes the 64-bit SimHash of a query string using character 3-grams.
pub fn compute_simhash(text: &str) -> u64 {
    let normalized: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();

    if chars.is_empty() {
        return 0;
    }

    let mut bit_counts = [0i32; 64];

    if chars.len() < 3 {
        let mut s = String::new();
        while s.chars().count() < 3 {
            s.push_str(&normalized);
        }
        let shingle_hash = fnv1a_64(&s);
        accumulate_hash(&mut bit_counts, shingle_hash);
    } else {
        for window in chars.windows(3) {
            let shingle: String = window.iter().collect();
            let shingle_hash = fnv1a_64(&shingle);
            accumulate_hash(&mut bit_counts, shingle_hash);
        }
    }

    let mut fingerprint: u64 = 0;
    for (i, &count) in bit_counts.iter().enumerate() {
        if count > 0 {
            fingerprint |= 1u64 << i;
        }
    }

    fingerprint
}

#[inline]
fn accumulate_hash(counts: &mut [i32; 64], hash: u64) {
    for i in 0..64 {
        if (hash >> i) & 1 == 1 {
            counts[i] += 1;
        } else {
            counts[i] -= 1;
        }
    }
}

/// 64-bit FNV-1a non-cryptographic hash for fast shingle fingerprinting.
#[inline]
fn fnv1a_64(s: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Returns the Hamming distance between two 64-bit SimHashes (0..64).
/// Executed as a single machine instruction `POPCNT` in ~0.3 nanoseconds.
#[inline]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Returns similarity percentage from 0.0% to 100.0%.
#[inline]
pub fn similarity(a: u64, b: u64) -> f32 {
    let dist = hamming_distance(a, b);
    ((64 - dist) as f32 / 64.0) * 100.0
}

/// Evaluates whether a candidate question matches a query, combining SimHash similarity
/// with a short-query semantic guard to prevent false positive cache hits
/// (e.g. "What is the capital of Mars?" vs "What is the capital of France?").
pub fn matches_query(query: &str, candidate_question: &str, min_similarity: f32) -> bool {
    let q_clean = query.trim();
    let c_clean = candidate_question.trim();
    if q_clean.is_empty() || c_clean.is_empty() {
        return false;
    }

    let h1 = compute_simhash(q_clean);
    let h2 = compute_simhash(c_clean);
    let sim = similarity(h1, h2);
    if sim < min_similarity {
        return false;
    }

    // Short-query semantic guard:
    // When queries have fewer than 8 words, common framing words ("What is the capital of...")
    // dominate the 3-gram SimHash. We verify that substantive non-stopword tokens also match.
    let q_words: Vec<&str> = q_clean.split_whitespace().collect();
    let c_words: Vec<&str> = c_clean.split_whitespace().collect();

    if q_words.len() < 8 || c_words.len() < 8 {
        const STOP_WORDS: &[&str] = &[
            "what", "is", "the", "of", "a", "an", "how", "to", "in", "on", "for", "and", "or",
            "are", "do", "does", "did", "can", "could", "would", "should", "it", "at", "by",
            "nedir", "nasil", "ne", "ve", "ile", "bir", "icin", "bu", "da", "de",
        ];

        let q_substantive: Vec<String> = q_words
            .iter()
            .map(|w| w.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.as_str()))
            .collect();

        let c_substantive: Vec<String> = c_words
            .iter()
            .map(|w| w.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.as_str()))
            .collect();

        if !q_substantive.is_empty() && !c_substantive.is_empty() {
            let matched_substantive = q_substantive
                .iter()
                .filter(|w| c_substantive.contains(w))
                .count();

            // At least 60% of query's substantive words must be present in candidate
            let overlap = (matched_substantive as f32) / (q_substantive.len() as f32);
            if overlap < 0.6 {
                return false;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_have_100_percent_similarity() {
        let q = "How does distributed P2P inference work?";
        let h1 = compute_simhash(q);
        let h2 = compute_simhash(q);
        assert_eq!(h1, h2);
        assert_eq!(hamming_distance(h1, h2), 0);
        assert_eq!(similarity(h1, h2), 100.0);
    }

    #[test]
    fn case_and_whitespace_are_normalized() {
        let h1 = compute_simhash("Rust Arc Mutex usage");
        let h2 = compute_simhash("  rust arc mutex usage  ");
        assert_eq!(h1, h2);
    }

    #[test]
    fn small_typos_and_suffixes_maintain_high_similarity() {
        // Turkish suffix variations
        let q1 = "Rust'ta Arc ile Mutex kullanımı";
        let q2 = "Rustta Arc ve Mutex kullanımı";
        let h1 = compute_simhash(q1);
        let h2 = compute_simhash(q2);
        let sim = similarity(h1, h2);
        assert!(
            sim >= 80.0,
            "expected high similarity >= 80%, got {sim}% (hamming dist: {})",
            hamming_distance(h1, h2)
        );

        // English minor variation
        let e1 = "What is the difference between TCP and UDP?";
        let e2 = "What is difference between TCP vs UDP?";
        let he1 = compute_simhash(e1);
        let he2 = compute_simhash(e2);
        let sim_e = similarity(he1, he2);
        assert!(
            sim_e >= 80.0,
            "expected high similarity >= 80%, got {sim_e}%"
        );
    }

    #[test]
    fn completely_unrelated_questions_have_low_similarity() {
        let q1 = "How does proof of work consensus function?";
        let q2 = "Recipe for homemade Italian sourdough pizza";
        let h1 = compute_simhash(q1);
        let h2 = compute_simhash(q2);
        let sim = similarity(h1, h2);
        assert!(
            sim < 70.0,
            "expected low similarity < 70%, got {sim}%"
        );
    }

    #[test]
    fn short_query_semantic_guard_rejects_distinct_nouns() {
        let q1 = "What is the capital of Mars?";
        let q2 = "What is the capital of France?";
        // Pure SimHash similarity is high (~89%), but substantive words ("mars" vs "france") differ
        assert!(!matches_query(q1, q2, 85.0));
    }

    #[test]
    fn short_query_semantic_guard_accepts_similar_intent() {
        let q1 = "What is the capital of France?";
        let q2 = "What is capital of France?";
        assert!(matches_query(q1, q2, 85.0));
    }
}
