//! Physical Real-Time Streaming Client for Cloud AI Providers.
//!
//! Streams tokens in real-time using HTTP Server-Sent Events (SSE) over ureq,
//! without requiring heavy async HTTP runtimes.

use std::io::{BufRead, BufReader, Read};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};

use crate::api::prompt::MBHUB_SYSTEM_PROMPT;
use crate::model::PROVIDERS;

#[derive(Clone, Debug)]
pub enum StreamMessage {
    Token(String),
    Done { full_text: String, is_truncated: bool },
    Error(String),
}

/// Hard ceiling for streamed provider output (audit O13): the answer text and
/// the reasoning scratchpad share this combined budget, so a misbehaving or
/// hostile endpoint cannot exhaust memory by ignoring `max_tokens` and
/// streaming forever.
pub const MAX_STREAM_OUTPUT_BYTES: usize = 1_048_576;

/// Hard ceiling for a single SSE line. Real provider events are a few hundred
/// bytes; anything near this size means the endpoint is broken or hostile and
/// the line must be cut off before it is buffered in full.
const MAX_SSE_LINE_BYTES: usize = MAX_STREAM_OUTPUT_BYTES;

/// Appends `next` to `current` while enforcing the cumulative stream output
/// ceiling ([`MAX_STREAM_OUTPUT_BYTES`]).
///
/// `total_len` is the number of bytes accumulated across ALL output buffers
/// (they share one budget) and `*truncated` is likewise shared: once set, all
/// further appends are refused.
///
/// Returns `true` when the full chunk fit. Returns `false` when the ceiling
/// was reached: only the UTF-8-safe prefix that still fits is appended,
/// `*truncated` is set, and the caller must stop consuming the stream.
fn enforce_output_cap(
    current: &mut String,
    next: &str,
    total_len: &mut usize,
    truncated: &mut bool,
) -> bool {
    if *truncated {
        return false;
    }
    let remaining = MAX_STREAM_OUTPUT_BYTES.saturating_sub(*total_len);
    if next.len() <= remaining {
        current.push_str(next);
        *total_len += next.len();
        return true;
    }
    // Keep only what fits, cut on a UTF-8 character boundary so the
    // accumulator always stays a valid String.
    let mut take = remaining;
    while take > 0 && !next.is_char_boundary(take) {
        take -= 1;
    }
    current.push_str(&next[..take]);
    *total_len = MAX_STREAM_OUTPUT_BYTES;
    *truncated = true;
    false
}

/// Spawns a background worker to stream tokens from the selected AI provider.
pub fn spawn_stream(
    provider_idx: usize,
    model: &str,
    api_key: &str,
    query: &str,
) -> Receiver<StreamMessage> {
    let (tx, rx) = unbounded();
    let provider = PROVIDERS[provider_idx];
    let provider_name = provider.name.to_string();
    let endpoint = provider.endpoint.to_string();
    let model = model.to_string();
    let api_key = api_key.to_string();
    let query = query.to_string();

    thread::Builder::new()
        .name("mbhub-stream".to_string())
        .spawn(move || {
            run_stream(tx, &provider_name, &endpoint, &model, &api_key, &query);
        })
        .expect("failed to spawn AI stream thread");

    rx
}

fn run_stream(
    tx: Sender<StreamMessage>,
    provider_name: &str,
    endpoint: &str,
    model: &str,
    api_key: &str,
    query: &str,
) {
    if api_key.trim().is_empty() {
        let _ = tx.send(StreamMessage::Error(
            "API key is not configured. Please set your API key in SETTINGS > Cloud AI provider."
                .to_string(),
        ));
        return;
    }

    let is_anthropic = provider_name.eq_ignore_ascii_case("Anthropic");
    let is_openrouter = provider_name.eq_ignore_ascii_case("OpenRouter");

    let (url, payload, auth_header_key, auth_header_val) = if is_anthropic {
        let url = "https://api.anthropic.com/v1/messages".to_string();
        let payload = json!({
            "model": model,
            "system": MBHUB_SYSTEM_PROMPT,
            "messages": [
                { "role": "user", "content": query }
            ],
            "max_tokens": 8192,
            "stream": true
        });
        (url, payload, "x-api-key", api_key.to_string())
    } else {
        let base = endpoint.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        let mut payload_map = serde_json::Map::new();
        payload_map.insert("model".to_string(), json!(model));
        payload_map.insert(
            "messages".to_string(),
            json!([
                { "role": "system", "content": MBHUB_SYSTEM_PROMPT },
                { "role": "user", "content": query }
            ]),
        );
        payload_map.insert("max_tokens".to_string(), json!(8192));
        payload_map.insert("stream".to_string(), json!(true));

        if is_openrouter {
            // Prevent OpenRouter reasoning models from wasting thousands of tokens
            // on raw scratchpad text that depletes budget and pollutes answers.
            payload_map.insert("include_reasoning".to_string(), json!(false));
        }

        let payload = Value::Object(payload_map);
        (url, payload, "Authorization", format!("Bearer {api_key}"))
    };

    let agent = ureq::builder()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300)) // 5 minutes inactivity tolerance between tokens
        .build();

    let mut req = agent
        .post(&url)
        .set(auth_header_key, &auth_header_val)
        .set("Content-Type", "application/json");

    if is_anthropic {
        req = req.set("anthropic-version", "2023-06-01");
    }

    let res = match req.send_json(payload) {
        Ok(res) => res,
        Err(ureq::Error::Status(code, res)) => {
            let body = res.into_string().unwrap_or_default();
            let _ = tx.send(StreamMessage::Error(format!(
                "Provider returned HTTP {code}: {body}"
            )));
            return;
        }
        Err(e) => {
            let _ = tx.send(StreamMessage::Error(format!("Connection failed: {e}")));
            return;
        }
    };

    let mut reader = BufReader::new(res.into_reader());
    let mut full_text = String::new();
    let mut reasoning_text = String::new();
    // Shared output budget (audit O13): the answer and the reasoning
    // scratchpad draw from ONE ceiling so a hostile endpoint cannot inflate
    // total memory by switching between the two fields.
    let mut output_len = 0usize;
    let mut output_capped = false;
    let mut finish_reason: Option<String> = None;
    let mut read_error: Option<String> = None;
    // Reusable line buffer: `read_until` appends, cleared every iteration.
    let mut raw_line: Vec<u8> = Vec::with_capacity(8 * 1024);

    // Manual SSE read loop instead of `reader.lines()`: every line is size
    // checked BEFORE it is buffered, so a single gigantic line from a broken
    // or hostile endpoint cannot balloon memory.
    loop {
        raw_line.clear();
        let n = {
            // `take` bounds how much of the line can be pulled into memory at
            // all; `read_line`/`lines()` would grow the buffer without limit.
            let mut limited = (&mut reader).take(MAX_SSE_LINE_BYTES as u64 + 1);
            match limited.read_until(b'\n', &mut raw_line) {
                Ok(0) => break, // EOF: stream finished cleanly
                Ok(n) => n,
                Err(e) => {
                    read_error = Some(e.to_string());
                    break;
                }
            }
        };
        if n as usize > MAX_SSE_LINE_BYTES {
            read_error = Some(format!(
                "SSE line exceeded the {}-byte safety ceiling",
                MAX_SSE_LINE_BYTES
            ));
            break;
        }
        // Invalid UTF-8 degrades to replacement characters (the JSON parsing
        // below ignores such lines) instead of aborting the whole stream.
        let line = String::from_utf8_lossy(&raw_line);

        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(':') {
            continue;
        }

        // Support both "data: {...}" and "data:{...}"
        if let Some(raw_data) = trimmed.strip_prefix("data:") {
            let data = raw_data.trim();
            if data == "[DONE]" {
                break;
            }

            if is_anthropic {
                if let Ok(val) = serde_json::from_str::<Value>(data) {
                    if let Some(event_type) = val.get("type").and_then(|t| t.as_str()) {
                        if event_type == "content_block_delta" {
                            if let Some(text) = val
                                .get("delta")
                                .and_then(|d| d.get("text"))
                                .and_then(|t| t.as_str())
                            {
                                // Output cap (audit O13): tokens past the
                                // ceiling are dropped, the caller stops
                                // reading after this line.
                                if enforce_output_cap(
                                    &mut full_text,
                                    text,
                                    &mut output_len,
                                    &mut output_capped,
                                ) {
                                    let _ = tx.send(StreamMessage::Token(text.to_string()));
                                }
                            }
                        } else if event_type == "message_delta" {
                            if let Some(stop_reason) = val
                                .get("delta")
                                .and_then(|d| d.get("stop_reason"))
                                .and_then(|s| s.as_str())
                            {
                                finish_reason = Some(stop_reason.to_string());
                            }
                        } else if event_type == "message_stop" {
                            break;
                        }
                    }
                }
            } else if let Ok(val) = serde_json::from_str::<Value>(data) {
                if let Some(choice) = val
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| arr.first())
                {
                    if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                        finish_reason = Some(fr.to_string());
                    }

                    if let Some(delta) = choice.get("delta") {
                        // 1. Direct answer content
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                            if !content.is_empty() {
                                // Output cap (audit O13): see the Anthropic
                                // branch above.
                                if enforce_output_cap(
                                    &mut full_text,
                                    content,
                                    &mut output_len,
                                    &mut output_capped,
                                ) {
                                    let _ = tx.send(StreamMessage::Token(content.to_string()));
                                }
                            }
                        }
                        // 2. Reasoning scratchpad (only capture as fallback, do NOT mix with answer)
                        else if let Some(reasoning) = delta
                            .get("reasoning_content")
                            .or_else(|| delta.get("reasoning"))
                            .and_then(|r| r.as_str())
                        {
                            if !reasoning.is_empty() {
                                enforce_output_cap(
                                    &mut reasoning_text,
                                    reasoning,
                                    &mut output_len,
                                    &mut output_capped,
                                );
                            }
                        }
                    }
                }
            }
        }

        // Producer-side cap enforcement (audit O13): once the ceiling is hit,
        // stop reading — and therefore stop producing tokens — entirely.
        if output_capped {
            break;
        }
    }

    let mut answer = if !full_text.trim().is_empty() {
        full_text
    } else if !reasoning_text.trim().is_empty() {
        reasoning_text
    } else {
        String::new()
    };

    if answer.trim().is_empty() {
        let err_msg = if let Some(err) = read_error {
            format!("Connection interrupted: {err}")
        } else {
            "AI provider returned an empty answer.".to_string()
        };
        let _ = tx.send(StreamMessage::Error(err_msg));
        return;
    }

    let mut is_truncated = false;
    if let Some(fr) = &finish_reason {
        if fr == "length" || fr == "max_tokens" {
            is_truncated = true;
        }
    }
    if output_capped {
        is_truncated = true;
    }

    if let Some(err) = read_error {
        is_truncated = true;
        answer.push_str(&format!("\n\n[⚠️ RESPONSE INCOMPLETE: Connection lost ({err})]"));
    } else if output_capped {
        // Audit O13: the truncation detector in p2p/protocol.rs refuses
        // content containing this marker, so capped output is rendered to the
        // LOCAL user only and can never be broadcast to P2P peers.
        answer.push_str("\n\n[⚠️ RESPONSE INCOMPLETE: Output limit reached]");
    } else if is_truncated {
        answer.push_str("\n\n[⚠️ RESPONSE INCOMPLETE: Model token limit reached]");
    }

    let _ = tx.send(StreamMessage::Done {
        full_text: answer,
        is_truncated,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASCII filler of exactly `n` bytes.
    fn chunk(n: usize) -> String {
        "a".repeat(n)
    }

    #[test]
    fn output_cap_passes_normal_chunks_through() {
        let mut text = String::new();
        let mut total = 0usize;
        let mut truncated = false;

        assert!(enforce_output_cap(&mut text, "Hello ", &mut total, &mut truncated));
        assert!(enforce_output_cap(&mut text, "world", &mut total, &mut truncated));
        assert_eq!(text, "Hello world");
        assert_eq!(total, 11);
        assert!(!truncated);
    }

    #[test]
    fn output_cap_cuts_accumulation_at_ceiling() {
        // Pre-fill the budget to three bytes below the ceiling.
        let mut text = chunk(MAX_STREAM_OUTPUT_BYTES - 3);
        let mut total = text.len();
        let mut truncated = false;

        // A chunk larger than the remaining budget: only the fitting prefix
        // is kept and the caller is told to stop consuming.
        let big = chunk(64);
        assert!(!enforce_output_cap(&mut text, &big, &mut total, &mut truncated));
        assert_eq!(text.len(), MAX_STREAM_OUTPUT_BYTES);
        assert_eq!(total, MAX_STREAM_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn output_cap_is_shared_between_answer_and_reasoning() {
        // Audit O13: the two accumulators draw from ONE budget, so filling
        // the reasoning scratchpad must consume the answer's headroom.
        let mut full_text = String::new();
        let mut reasoning_text = String::new();
        let mut total = 0usize;
        let mut truncated = false;

        let first = MAX_STREAM_OUTPUT_BYTES / 2 + 100;
        assert!(enforce_output_cap(
            &mut full_text,
            &chunk(first),
            &mut total,
            &mut truncated
        ));
        assert!(!truncated);

        let remaining = MAX_STREAM_OUTPUT_BYTES - first;
        assert!(!enforce_output_cap(
            &mut reasoning_text,
            &chunk(remaining + 1),
            &mut total,
            &mut truncated
        ));
        assert_eq!(reasoning_text.len(), remaining);
        assert_eq!(total, MAX_STREAM_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn output_cap_keeps_utf8_valid_when_cut_mid_character() {
        let mut text = String::new();
        let mut total = MAX_STREAM_OUTPUT_BYTES - 3;
        let mut truncated = false;

        // 'ä' is 2 bytes: a naive 3-byte cut would land inside a character.
        let multibyte = "äää".to_string();
        assert!(!enforce_output_cap(&mut text, &multibyte, &mut total, &mut truncated));
        assert!(text.is_char_boundary(text.len()));
        assert_eq!(text, "ä");
        assert!(truncated);
    }

    #[test]
    fn output_cap_refuses_appends_after_truncation() {
        let mut text = String::new();
        let mut total = MAX_STREAM_OUTPUT_BYTES;
        let mut truncated = true; // already flagged

        assert!(!enforce_output_cap(&mut text, "more", &mut total, &mut truncated));
        assert!(text.is_empty());
        assert_eq!(total, MAX_STREAM_OUTPUT_BYTES);
    }

    #[test]
    fn output_cap_flags_truncation_only_when_exceeded() {
        let mut text = String::new();
        let mut total = 0usize;
        let mut truncated = false;

        // Filling the budget exactly is still a clean (non-truncated) append.
        assert!(enforce_output_cap(
            &mut text,
            &chunk(MAX_STREAM_OUTPUT_BYTES),
            &mut total,
            &mut truncated
        ));
        assert!(!truncated);
        // The next byte trips it.
        assert!(!enforce_output_cap(&mut text, "x", &mut total, &mut truncated));
        assert!(truncated);
    }
}
