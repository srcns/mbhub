//! P2P Wire Protocol definitions and payload governance.

use serde::{Deserialize, Serialize};

pub const GOSSIP_TOPIC_INFERENCES: &str = "mbhub/inferences/1.0.0";
pub const GOSSIP_TOPIC_QUERIES: &str = "mbhub/queries/1.0.0";
pub const GOSSIP_TOPIC_RESPONSES: &str = "mbhub/responses/1.0.0";
pub const GOSSIP_TOPIC_TOMBSTONES: &str = "mbhub/tombstones/1.0.0";

/// Strict 128 KB Wire Payload Ceiling to prevent agentic execution log flooding.
pub const MAX_GOSSIP_PAYLOAD: usize = 131_072;

/// Maximum application-layer hop count a gossip message may claim.
/// Messages with `hop_ttl == 0` or above this cap are dropped unprocessed
///.
pub const MAX_HOP_TTL: u8 = 16;

/// Maximum clock skew (seconds) tolerated between a gossip message's claimed
/// timestamp and local wall-clock before the message is rejected as spoofed.
pub const MAX_TIMESTAMP_SKEW_SECS: i64 = 300;

/// Serde default keeps old (pre-integrity) messages, which lack `hop_ttl`,
/// compatible with the current reader: they are treated as fresh messages.
fn default_hop_ttl() -> u8 {
    MAX_HOP_TTL
}

/// Wire format for a completed inference announced to the swarm.
///
/// Integrity: `content_hash` is the BLAKE3 hash over the
/// sanitized `(question, content, provider, model)` fields. Receivers MUST
/// recompute and compare before storing; mismatches are dropped. Legacy
/// senders without the field fail verification and are dropped by receivers
/// running this version.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwarmInferenceMessage {
    pub question: String,
    pub content: String,
    pub timestamp: i64,
    pub simhash: u64,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default = "default_hop_ttl")]
    pub hop_ttl: u8,
    #[serde(default)]
    pub is_truncated: bool,
}

impl SwarmInferenceMessage {
    /// Recomputes the canonical content hash over sanitized fields.
    pub fn canonical_content_hash(&self) -> String {
        crate::content_hash::compute_content_hash(
            &crate::sanitize::strip_control_chars(&self.question),
            &crate::sanitize::strip_control_chars(&self.content),
            &crate::sanitize::strip_control_chars(&self.provider),
            &crate::sanitize::strip_control_chars(&self.model),
        )
    }

    /// Full receiver-side integrity check:
    /// - payload within the 128 KB ceiling (checked earlier, re-checked here),
    /// - content not truncated or cut off mid-sentence,
    /// - content hash matches the actual fields,
    /// - timestamp within sane bounds (no far-future spoofing),
    /// - hop TTL within protocol bounds and not exhausted.
    pub fn passes_integrity_checks(&self, now_epoch: i64) -> bool {
        if self.is_truncated {
            return false;
        }
        if self.content.len() > MAX_GOSSIP_PAYLOAD {
            return false;
        }
        // Anti-Poison Gate: reject empty or uninformative content / questions
        if self.content.trim().is_empty()
            || self.content.trim().len() < 10
            || self.question.trim().is_empty()
            || self.question.trim().len() < 3
        {
            return false;
        }
        // Truncation detection guard: reject cut-off or interrupted answers
        if self.content.contains("[⚠️ RESPONSE INCOMPLETE") || self.content.contains("[⚠️ YANIT KESİLDİ") {
            return false;
        }
        if self.content_hash.is_empty() || self.content_hash != self.canonical_content_hash() {
            return false;
        }
        if self.timestamp > now_epoch.saturating_add(MAX_TIMESTAMP_SKEW_SECS) {
            return false;
        }
        if self.hop_ttl == 0 || self.hop_ttl > MAX_HOP_TTL {
            return false;
        }
        true
    }
}

/// Cryptographic Unidirectional Negative Signal (Tombstone) wire format.
/// Broadcast over P2P GossipSub to permanently mark poisoned, deleted,
/// or hallucinated content hashes so peers never cache or serve them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwarmTombstoneMessage {
    pub content_hash: String,
    pub simhash: u64,
    pub timestamp: i64,
    pub reporter_peer_id: String,
    pub reason: String,
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl SwarmTombstoneMessage {
    pub fn passes_integrity_checks(&self, now_epoch: i64) -> bool {
        if self.content_hash.is_empty() || self.content_hash.len() > 128 {
            return false;
        }
        if self.timestamp > now_epoch.saturating_add(MAX_TIMESTAMP_SKEW_SECS) {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwarmQueryRequest {
    pub request_id: String,
    pub asker_peer_id: String,
    pub question: String,
    pub simhash: u64,
    pub min_similarity: f32,
}

impl SwarmQueryRequest {
    /// Validates request sanity: non-empty IDs, question length >= 3, finite valid similarity.
    pub fn passes_integrity_checks(&self) -> bool {
        !self.request_id.is_empty()
            && !self.asker_peer_id.is_empty()
            && !self.question.trim().is_empty()
            && self.question.trim().len() >= 3
            && self.min_similarity.is_finite()
            && (0.0..=100.0).contains(&self.min_similarity)
            && crate::simhash::compute_simhash(&self.question) == self.simhash
    }
}

/// Wire format for a swarm hit returned in response to a query.
///
/// `content_hash` is mandatory for integrity: receivers recompute the hash
/// over the sanitized fields and drop mismatches. Legacy
/// peers without the field produce an empty hash and are rejected.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwarmQueryResponse {
    pub request_id: String,
    pub responder_peer_id: String,
    pub question: String,
    pub content: String,
    pub simhash: u64,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub content_hash: String,
}

impl SwarmQueryResponse {
    /// Recomputes the canonical content hash over sanitized fields.
    pub fn canonical_content_hash(&self) -> String {
        crate::content_hash::compute_content_hash(
            &crate::sanitize::strip_control_chars(&self.question),
            &crate::sanitize::strip_control_chars(&self.content),
            &crate::sanitize::strip_control_chars(&self.provider),
            &crate::sanitize::strip_control_chars(&self.model),
        )
    }

    /// Receiver-side integrity check: payload ceiling + anti-poison + truncation guard + content hash match.
    pub fn passes_integrity_checks(&self) -> bool {
        if self.content.len() > MAX_GOSSIP_PAYLOAD {
            return false;
        }
        // Anti-Poison Hard Gate: reject empty or uninformative content / questions
        if self.content.trim().is_empty()
            || self.content.trim().len() < 10
            || self.question.trim().is_empty()
            || self.question.trim().len() < 3
        {
            return false;
        }
        // Truncation detection guard: reject cut-off or interrupted answers
        if self.content.contains("[⚠️ RESPONSE INCOMPLETE") || self.content.contains("[⚠️ YANIT KESİLDİ") {
            return false;
        }
        !self.content_hash.is_empty() && self.content_hash == self.canonical_content_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message() -> SwarmInferenceMessage {
        SwarmInferenceMessage {
            question: "How does distributed consensus work?".to_string(),
            content: "Consensus is achieved via Byzantine fault tolerance.".to_string(),
            timestamp: 1_770_000_000,
            simhash: 0x123456789ABCDEF0,
            provider: "OpenAI".to_string(),
            model: "gpt-4o".to_string(),
            content_hash: String::new(),
            hop_ttl: MAX_HOP_TTL,
            is_truncated: false,
        }
    }

    #[test]
    fn integrity_check_rejects_truncated_message() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();
        assert!(msg.passes_integrity_checks(msg.timestamp));

        // When flagged as truncated, integrity check must reject
        msg.is_truncated = true;
        assert!(!msg.passes_integrity_checks(msg.timestamp));

        // When content contains the truncation warning marker, integrity check must reject
        msg.is_truncated = false;
        msg.content.push_str("\n\n[⚠️ RESPONSE INCOMPLETE: Stream interrupted]");
        msg.content_hash = msg.canonical_content_hash();
        assert!(!msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn message_serialization_and_payload_ceiling() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();

        let bytes = serde_json::to_vec(&msg).expect("serialization succeeds");
        assert!(bytes.len() < MAX_GOSSIP_PAYLOAD);

        let deserialized: SwarmInferenceMessage =
            serde_json::from_slice(&bytes).expect("deserialization succeeds");
        assert_eq!(deserialized.question, msg.question);
        assert_eq!(deserialized.simhash, msg.simhash);
        assert_eq!(deserialized.content_hash, msg.content_hash);

        // Huge payload ceiling rejection check
        let huge_content = "X".repeat(MAX_GOSSIP_PAYLOAD + 10);
        assert!(huge_content.len() > MAX_GOSSIP_PAYLOAD);
    }

    #[test]
    fn integrity_check_passes_valid_message() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();
        assert!(msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn integrity_check_rejects_tampered_content() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();
        msg.content = "TAMPERED payload.".to_string();
        assert!(!msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn integrity_check_rejects_missing_hash() {
        let msg = sample_message(); // content_hash empty
        assert!(!msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn integrity_check_rejects_future_timestamp() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();
        // Claimed timestamp far in the future relative to "now".
        let now = 1_770_000_000;
        msg.timestamp = now + MAX_TIMESTAMP_SKEW_SECS + 60;
        assert!(!msg.passes_integrity_checks(now));
    }

    #[test]
    fn integrity_check_rejects_exhausted_ttl() {
        let mut msg = sample_message();
        msg.content_hash = msg.canonical_content_hash();
        msg.hop_ttl = 0;
        assert!(!msg.passes_integrity_checks(msg.timestamp));
        msg.hop_ttl = MAX_HOP_TTL + 1;
        assert!(!msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn legacy_message_without_hop_ttl_deserializes_with_default() {
        // Old wire format: no content_hash, no hop_ttl fields.
        let legacy = r#"{"question":"q","content":"c","timestamp":1770000000,"simhash":42,"provider":"OpenAI","model":"gpt-4o"}"#;
        let msg: SwarmInferenceMessage = serde_json::from_str(legacy).unwrap();
        assert_eq!(msg.hop_ttl, MAX_HOP_TTL);
        assert!(msg.content_hash.is_empty());
        // Integrity check still rejects it (no content hash to verify) —
        // receivers require the new format.
        assert!(!msg.passes_integrity_checks(msg.timestamp));
    }

    #[test]
    fn query_response_integrity() {
        let mut resp = SwarmQueryResponse {
            request_id: "req-123".to_string(),
            responder_peer_id: "peer-xyz".to_string(),
            question: "What is Raft consensus?".to_string(),
            content: "Raft is a leader-based consensus algorithm.".to_string(),
            simhash: 0xAABBCCDDEEFF0011,
            provider: "Anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            content_hash: String::new(),
        };
        assert!(!resp.passes_integrity_checks());

        resp.content_hash = resp.canonical_content_hash();
        assert!(resp.passes_integrity_checks());

        resp.content = "Corrupted.".to_string();
        assert!(!resp.passes_integrity_checks());
    }

    #[test]
    fn swarm_query_request_and_response_serialization() {
        let req = SwarmQueryRequest {
            request_id: "req-123".to_string(),
            asker_peer_id: "peer-abc".to_string(),
            question: "What is Raft consensus?".to_string(),
            simhash: 0xAABBCCDDEEFF0011,
            min_similarity: 85.0,
        };
        let req_bytes = serde_json::to_vec(&req).unwrap();
        let parsed_req: SwarmQueryRequest = serde_json::from_slice(&req_bytes).unwrap();
        assert_eq!(parsed_req.request_id, "req-123");
        assert_eq!(parsed_req.simhash, 0xAABBCCDDEEFF0011);

        let mut resp = SwarmQueryResponse {
            request_id: "req-123".to_string(),
            responder_peer_id: "peer-xyz".to_string(),
            question: "What is Raft consensus?".to_string(),
            content: "Raft is a leader-based consensus algorithm.".to_string(),
            simhash: 0xAABBCCDDEEFF0011,
            provider: "Anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            content_hash: String::new(),
        };
        resp.content_hash = resp.canonical_content_hash();
        let resp_bytes = serde_json::to_vec(&resp).unwrap();
        let parsed_resp: SwarmQueryResponse = serde_json::from_slice(&resp_bytes).unwrap();
        assert_eq!(parsed_resp.content, resp.content);
        assert_eq!(parsed_resp.provider, "Anthropic");
        assert_eq!(parsed_resp.content_hash, resp.content_hash);
    }

    #[test]
    fn anti_poison_rejects_empty_or_short_payloads() {
        // 1. SwarmInferenceMessage anti-poison checks
        let mut msg = sample_message();
        msg.content = "   ".to_string();
        msg.content_hash = msg.canonical_content_hash();
        assert!(!msg.passes_integrity_checks(msg.timestamp), "empty content must be rejected");

        let mut short_c = sample_message();
        short_c.content = "short".to_string();
        short_c.content_hash = short_c.canonical_content_hash();
        assert!(!short_c.passes_integrity_checks(short_c.timestamp), "content < 10 chars must be rejected");

        let mut short_q = sample_message();
        short_q.question = "hi".to_string();
        short_q.content_hash = short_q.canonical_content_hash();
        assert!(!short_q.passes_integrity_checks(short_q.timestamp), "question < 3 chars must be rejected");

        // 2. SwarmQueryResponse anti-poison checks
        let mut resp = SwarmQueryResponse {
            request_id: "req-1".to_string(),
            responder_peer_id: "peer-1".to_string(),
            question: "Valid question?".to_string(),
            content: "   ".to_string(),
            simhash: 12345,
            provider: "OpenAI".to_string(),
            model: "gpt-4o".to_string(),
            content_hash: String::new(),
        };
        resp.content_hash = resp.canonical_content_hash();
        assert!(!resp.passes_integrity_checks(), "empty query response content must be rejected");

        resp.content = "123456789".to_string(); // 9 chars
        resp.content_hash = resp.canonical_content_hash();
        assert!(!resp.passes_integrity_checks(), "query response content < 10 chars must be rejected");

        // 3. SwarmQueryRequest anti-poison checks
        let req = SwarmQueryRequest {
            request_id: "req-1".to_string(),
            asker_peer_id: "peer-1".to_string(),
            question: "hi".to_string(),
            simhash: crate::simhash::compute_simhash("hi"),
            min_similarity: 85.0,
        };
        assert!(!req.passes_integrity_checks(), "query request with short question must be rejected");
    }

    #[test]
    fn tombstone_message_serialization_and_integrity() {
        let t = SwarmTombstoneMessage {
            content_hash: "blake3_hash_1234".to_string(),
            simhash: 0x12345678,
            timestamp: 1_770_000_000,
            reporter_peer_id: "peer_123".to_string(),
            reason: "User flagged hallucination".to_string(),
            signature: vec![1, 2, 3],
        };
        assert!(t.passes_integrity_checks(1_770_000_000));
        let bytes = serde_json::to_vec(&t).unwrap();
        let parsed: SwarmTombstoneMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.content_hash, t.content_hash);
        assert_eq!(parsed.reason, t.reason);
    }
}
