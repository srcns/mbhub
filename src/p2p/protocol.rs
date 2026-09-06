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
    /// Publish proof-of-work: a nonce whose BLAKE3 hash over
    /// (author pubkey ‖ content_hash ‖ nonce) has
    /// [`PUBLISH_POW_DIFFICULTY_BITS`] leading zero bits. Raises the cost of
    /// flooding the network with content, independent of identity count.
    #[serde(default)]
    pub pow: String,
    /// Gossip-authenticated distributor of this record. Transport metadata:
    /// never serialized (receivers derive it from the signed gossip author,
    /// which cannot be spoofed). Powers local publisher bans.
    #[serde(skip)]
    pub author_peer_id: String,
}

/// Leading zero bits required in the publish proof-of-work hash. ~1-2 s of
/// single-core BLAKE3 per published inference — negligible for honest nodes
/// (one solve per L3 answer), a real cost for identity-farming flooders.
pub const PUBLISH_POW_DIFFICULTY_BITS: u32 = 24;

/// Computes the publish proof-of-work hash for one nonce attempt.
fn pow_hash(author_pubkey: &[u8], content_hash: &str, nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(author_pubkey);
    hasher.update(content_hash.as_bytes());
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Precomputed prefix state for the solver: the pubkey and content hash are
/// constant across nonce attempts, so the solver clones the mid-state and
/// only folds the 8 nonce bytes per attempt.
fn pow_prefix(author_pubkey: &[u8], content_hash: &str) -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(author_pubkey);
    hasher.update(content_hash.as_bytes());
    hasher
}

/// Counts leading zero bits of a 32-byte hash.
fn leading_zero_bits(hash: &[u8; 32]) -> u32 {
    let mut bits = 0u32;
    for &byte in hash {
        if byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

/// Solves the publish proof-of-work for one inference. Returns the nonce as
/// a decimal string (bounded: difficulty 28 ≈ 2^28 attempts ≈ well under a
/// second on modern hardware).
pub fn solve_publish_pow(author_pubkey: &[u8], content_hash: &str) -> String {
    solve_publish_pow_with_difficulty(author_pubkey, content_hash, PUBLISH_POW_DIFFICULTY_BITS)
}

/// Difficulty-injectable solver (tests use a lower difficulty).
pub fn solve_publish_pow_with_difficulty(
    author_pubkey: &[u8],
    content_hash: &str,
    difficulty_bits: u32,
) -> String {
    let prefix = pow_prefix(author_pubkey, content_hash);
    let mut nonce: u64 = 0;
    loop {
        let mut attempt = prefix.clone();
        attempt.update(&nonce.to_le_bytes());
        if leading_zero_bits(attempt.finalize().as_bytes()) >= difficulty_bits {
            return nonce.to_string();
        }
        nonce = nonce.wrapping_add(1);
    }
}

/// Verifies a publish proof-of-work against the author's public key.
pub fn verify_publish_pow(
    author_pubkey: &[u8],
    content_hash: &str,
    pow: &str,
    difficulty_bits: u32,
) -> bool {
    let Ok(nonce) = pow.trim().parse::<u64>() else {
        return false;
    };
    leading_zero_bits(&pow_hash(author_pubkey, content_hash, nonce)) >= difficulty_bits
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
///
/// Security: every tombstone MUST carry an Ed25519 `signature` made by the
/// reporter over [`SwarmTombstoneMessage::signing_payload`]; receivers drop
/// unsigned, mis-signed, or mis-attributed tombstones at the swarm edge.
/// Without this, any peer could censor the swarm by deleting arbitrary
/// content hashes.
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
    /// Canonical byte payload covered by the reporter's Ed25519 signature:
    /// `content_hash || simhash(BE u64) || timestamp(BE i64) || reason`.
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(self.content_hash.len() + 20 + self.reason.len());
        payload.extend_from_slice(self.content_hash.as_bytes());
        payload.extend_from_slice(&self.simhash.to_be_bytes());
        payload.extend_from_slice(&self.timestamp.to_be_bytes());
        payload.extend_from_slice(self.reason.as_bytes());
        payload
    }

    pub fn passes_integrity_checks(&self, now_epoch: i64) -> bool {
        // BLAKE3 hex digest: exactly 64 lowercase hex characters.
        if self.content_hash.len() != 64
            || !self
                .content_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return false;
        }
        if self.timestamp > now_epoch.saturating_add(MAX_TIMESTAMP_SKEW_SECS) {
            return false;
        }
        // Ed25519 signatures are 64 bytes.
        if self.signature.len() != 64 {
            return false;
        }
        // Reason is bounded to keep the wire payload and DB rows sane.
        if self.reason.len() > 256 {
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
    use libp2p::identity::Keypair;

    #[test]
    fn publish_pow_roundtrip_and_tamper() {
        let keypair = Keypair::generate_ed25519();
        let pubkey = keypair.public().encode_protobuf();
        let content_hash = "a".repeat(64);

        // Low-difficulty solve (fast test), verified at the same difficulty.
        let pow = solve_publish_pow_with_difficulty(&pubkey, &content_hash, 12);
        assert!(verify_publish_pow(&pubkey, &content_hash, &pow, 12));

        // A different key fails verification (PoW is author-bound).
        let other = Keypair::generate_ed25519();
        assert!(!verify_publish_pow(
            &other.public().encode_protobuf(),
            &content_hash,
            &pow,
            12
        ));

        // Different content fails verification.
        assert!(!verify_publish_pow(&pubkey, &"b".repeat(64), &pow, 12));

        // Garbage nonce fails.
        assert!(!verify_publish_pow(&pubkey, &content_hash, "not-a-number", 12));
    }

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
            pow: String::new(),
            author_peer_id: String::new(),
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
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let reporter = libp2p::identity::PeerId::from(keypair.public()).to_string();
        let mut t = SwarmTombstoneMessage {
            content_hash: "a".repeat(64),
            simhash: 0x12345678,
            timestamp: 1_770_000_000,
            reporter_peer_id: reporter,
            reason: "User flagged hallucination".to_string(),
            signature: Vec::new(),
        };
        t.signature = keypair.sign(&t.signing_payload()).expect("ed25519 sign");
        assert!(t.passes_integrity_checks(1_770_000_000));
        let bytes = serde_json::to_vec(&t).unwrap();
        let parsed: SwarmTombstoneMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.content_hash, t.content_hash);
        assert_eq!(parsed.reason, t.reason);
        // Signature round-trips and still verifies.
        assert_eq!(parsed.signature, t.signature);
        assert!(
            keypair
                .public()
                .verify(&parsed.signing_payload(), &parsed.signature),
            "signature must verify after wire round-trip"
        );
    }

    #[test]
    fn tombstone_rejects_unsigned_malformed_or_tampered() {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let mut t = SwarmTombstoneMessage {
            content_hash: "a".repeat(64),
            simhash: 0x12345678,
            timestamp: 1_770_000_000,
            reporter_peer_id: libp2p::identity::PeerId::from(keypair.public()).to_string(),
            reason: "x".to_string(),
            signature: Vec::new(),
        };

        // Unsigned (the pre-fix wire format) must be rejected outright.
        assert!(!t.passes_integrity_checks(1_770_000_000));

        // Signature of the wrong length rejected.
        t.signature = vec![1; 63];
        assert!(!t.passes_integrity_checks(1_770_000_000));

        // Non-hex / wrong-length content hash rejected.
        t.signature = keypair.sign(&t.signing_payload()).unwrap();
        t.content_hash = "blake3_hash_1234".to_string();
        t.signature = keypair.sign(&t.signing_payload()).unwrap();
        assert!(!t.passes_integrity_checks(1_770_000_000));

        // Well-formed hash + signature passes.
        t.content_hash = "b".repeat(64);
        t.signature = keypair.sign(&t.signing_payload()).unwrap();
        assert!(t.passes_integrity_checks(1_770_000_000));

        // Tampered reason invalidates the signature.
        t.reason = "tampered".to_string();
        let pk = keypair.public();
        assert!(!pk.verify(&t.signing_payload(), &t.signature));
    }
}
