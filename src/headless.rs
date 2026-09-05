//! Headless query execution engine for MBHub.
//!
//! Executes the 3-layer answer pipeline without a TUI:
//! L1 Local SQLite -> L2 P2P Swarm -> L3 BYOK Cloud Model.
//!
//! Used by:
//! - `mbhub ask <query>` (CLI / Shell)
//! - `mbhub daemon` (Background IPC Service)
//! - `mbhub mcp` (Model Context Protocol server)

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::thread;

use crate::ipc::IpcResponse;
use crate::model::{Settings, PROVIDERS};
use crate::p2p::protocol::{SwarmInferenceMessage, SwarmQueryRequest};
use crate::p2p::P2pHandle;

/// Executes an atomic ask query through the full 3-layer pipeline.
pub fn execute_ask(query: &str, p2p: Option<&P2pHandle>) -> Result<IpcResponse, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("Query cannot be empty.".to_string());
    }

    // Gate 1: DLP Pre-Flight Gate
    let dlp = crate::dlp::scan_text(trimmed);
    if dlp.is_sensitive {
        let pattern = dlp.matched_pattern.unwrap_or("sensitive pattern");
        return Err(format!(
            "DLP Blocked: Sensitive data detected ({}). Query not sent.",
            pattern
        ));
    }

    // Compute 64-bit SimHash and record profile for Query Locality
    let q_simhash = crate::simhash::compute_simhash(trimmed);
    let settings = Settings::load();
    let min_sim = settings.hit_rate.percentage();
    crate::db::record_profile_query(q_simhash);

    // Gate 2: L1 Local SQLite Memory lookup (with short-query guard and tombstone exclusion)
    let min_ts = settings
        .freshness
        .min_timestamp(chrono::Local::now().timestamp());

    if let Some(cached) = crate::db::find_best_match_query_fresh(trimmed, min_sim, min_ts) {
        return Ok(IpcResponse::Answer {
            question: cached.question,
            content: cached.content,
            source: if cached.is_swarm {
                "L2 (swarm cached)".to_string()
            } else {
                "L1 (local SQLite)".to_string()
            },
            similarity: cached.similarity as f64,
            is_swarm: cached.is_swarm,
        });
    }

    // Gate 3: L2 P2P Swarm Lookup (if peers are connected)
    if let Some(p) = p2p {
        if p.connected_peers() > 0 {
            let request_id = format!(
                "{:x}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            p.broadcast_query(SwarmQueryRequest {
                request_id: request_id.clone(),
                asker_peer_id: p.peer_id(),
                question: trimmed.to_string(),
                simhash: q_simhash,
                min_similarity: min_sim,
            });

            // Wait up to 600 ms deadline for swarm response
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                if let Ok(resp) = p.query_response_rx.try_recv() {
                    if resp.request_id == request_id {
                        // Anti-Poison Hard Gate: reject answerless or short content
                        if resp.content.trim().is_empty()
                            || resp.content.trim().len() < 10
                            || resp.question.trim().is_empty()
                            || resp.question.trim().len() < 3
                        {
                            continue;
                        }
                        let is_honest = resp.simhash == q_simhash
                            && resp.passes_integrity_checks();

                        if is_honest {
                            let inbound_dlp = crate::dlp::scan_text(&resp.content);
                            let inbound_safety =
                                crate::content_safety::screen_text(&resp.content);

                            if !inbound_dlp.is_sensitive
                                && inbound_safety.is_allowed()
                                && !crate::db::is_tombstoned(&resp.content_hash)
                            {
                                crate::db::save_swarm_inference(
                                    &resp.question,
                                    &resp.content,
                                    resp.simhash,
                                    &resp.provider,
                                    &resp.model,
                                    &resp.content_hash,
                                );

                                return Ok(IpcResponse::Answer {
                                    question: resp.question,
                                    content: resp.content,
                                    source: "L2 (swarm P2P)".to_string(),
                                    similarity: min_sim as f64,
                                    is_swarm: true,
                                });
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(15));
            }
        }
    }

    // Gate 4: L3 Cloud AI Model Fallback (BYOK)
    let provider_idx = settings.provider_idx.min(PROVIDERS.len() - 1);
    let provider = PROVIDERS[provider_idx];
    let model = if !settings.provider_model.is_empty() {
        settings.provider_model.clone()
    } else {
        "gpt-4o".to_string()
    };
    let api_key = settings.api_key.trim().to_string();

    if api_key.is_empty() {
        let env_var = crate::env::provider_to_env_var(provider.name);
        return Err(format!(
            "API Key Required: No cached answer reached threshold ({}) in P2P swarm, and no API key is configured for {}.\nConfigure via `mbhub` settings or export {}=\"...\"",
            settings.hit_rate.label(),
            provider.name,
            env_var
        ));
    }

    let rx = crate::api::stream::spawn_stream(provider_idx, &model, &api_key, trimmed);
    let mut full_response = String::new();
    let is_stream_truncated;

    // Collect stream with generous 300s token inactivity timeout
    loop {
        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(crate::api::stream::StreamMessage::Token(token)) => {
                full_response.push_str(&token);
            }
            Ok(crate::api::stream::StreamMessage::Done { full_text, is_truncated }) => {
                full_response = full_text;
                is_stream_truncated = is_truncated;
                break;
            }
            Ok(crate::api::stream::StreamMessage::Error(err)) => {
                if !full_response.is_empty() {
                    full_response.push_str(&format!("\n\n[⚠️ RESPONSE INCOMPLETE: {err}]"));
                    is_stream_truncated = true;
                    break;
                }
                return Err(format!("AI Provider Error: {}", err));
            }
            Err(_) => {
                if !full_response.is_empty() {
                    full_response.push_str("\n\n[⚠️ RESPONSE INCOMPLETE: Timeout]");
                    is_stream_truncated = true;
                    break;
                }
                return Err("AI Provider Error: Request timed out after 300 seconds.".to_string());
            }
        }
    }

    let redacted_response = crate::dlp::redact_secrets(&full_response);
    if redacted_response.trim().is_empty() || redacted_response.trim().len() < 10 {
        return Err("AI Provider Error: Model returned an empty or insufficient response.".to_string());
    }

    let now_ts = chrono::Local::now().timestamp();

    // Save inference locally (unlimited size, records whether it was truncated)
    if crate::db::save_inference_with_truncated(
        trimmed,
        &redacted_response,
        q_simhash,
        provider.name,
        &model,
        is_stream_truncated,
    ).is_none() {
        return Err("Security Gate Error: Response rejected by database security gate.".to_string());
    }

    // Broadcast to swarm if connected AND not truncated AND passing anti-poison gate AND within 128 KB wire ceiling
    if let Some(p) = p2p {
        if !is_stream_truncated
            && !redacted_response.trim().is_empty()
            && redacted_response.trim().len() >= 10
            && !trimmed.trim().is_empty()
            && trimmed.trim().len() >= 3
            && redacted_response.len() <= crate::p2p::MAX_GOSSIP_PAYLOAD
        {
            let content_hash = crate::content_hash::compute_content_hash(
                trimmed,
                &redacted_response,
                provider.name,
                &model,
            );

            let msg = SwarmInferenceMessage {
                timestamp: now_ts,
                simhash: q_simhash,
                question: trimmed.to_string(),
                content: redacted_response.clone(),
                provider: provider.name.to_string(),
                model: model.clone(),
                content_hash,
                hop_ttl: 8,
                is_truncated: false,
            };

            // Outbound safety filter
            if crate::content_safety::screen_text(&msg.content).is_allowed() {
                p.broadcast_inference(msg);
            }
        }
    }

    Ok(IpcResponse::Answer {
        question: trimmed.to_string(),
        content: redacted_response,
        source: format!("L3 ({}/{})", provider.name, model),
        similarity: 100.0,
        is_swarm: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_empty_query_rejected() {
        let res = execute_ask("   ", None);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn headless_dlp_sensitive_query_blocked() {
        let res = execute_ask("My secret api key is sk-ant-api03-abcdef1234567890", None);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("DLP Blocked"));
    }
}
