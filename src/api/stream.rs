//! Physical Real-Time Streaming Client for Cloud AI Providers.
//!
//! Streams tokens in real-time using HTTP Server-Sent Events (SSE) over ureq,
//! without requiring heavy async HTTP runtimes.

use std::io::{BufRead, BufReader};
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

    let reader = BufReader::new(res.into_reader());
    let mut full_text = String::new();
    let mut reasoning_text = String::new();
    let mut finish_reason: Option<String> = None;
    let mut read_error: Option<String> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                read_error = Some(e.to_string());
                break;
            }
        };

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
                                full_text.push_str(text);
                                let _ = tx.send(StreamMessage::Token(text.to_string()));
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
                                full_text.push_str(content);
                                let _ = tx.send(StreamMessage::Token(content.to_string()));
                            }
                        }
                        // 2. Reasoning scratchpad (only capture as fallback, do NOT mix with answer)
                        else if let Some(reasoning) = delta
                            .get("reasoning_content")
                            .or_else(|| delta.get("reasoning"))
                            .and_then(|r| r.as_str())
                        {
                            if !reasoning.is_empty() {
                                reasoning_text.push_str(reasoning);
                            }
                        }
                    }
                }
            }
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

    if let Some(err) = read_error {
        is_truncated = true;
        answer.push_str(&format!("\n\n[⚠️ RESPONSE INCOMPLETE: Connection lost ({err})]"));
    } else if is_truncated {
        answer.push_str("\n\n[⚠️ RESPONSE INCOMPLETE: Model token limit reached]");
    }

    let _ = tx.send(StreamMessage::Done {
        full_text: answer,
        is_truncated,
    });
}
