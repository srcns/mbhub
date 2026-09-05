//! Physical AI Provider & Local AI API client.
//! Handles model discovery from endpoints and strict text-only filtering.

use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize, Debug)]
struct OpenAIModelItem {
    id: String,
}

#[derive(Deserialize, Debug)]
struct OpenAIModelListResponse {
    #[serde(default)]
    data: Vec<OpenAIModelItem>,
}

/// Blacklist keywords for non-text / multimodal / embedding / audio / vision models.
/// MBHub strictly accepts text-only output models.
const NON_TEXT_KEYWORDS: &[&str] = &[
    "whisper",
    "tts",
    "dall-e",
    "dalle",
    "flux",
    "stable-diffusion",
    "sdxl",
    "embed",
    "embedding",
    "moderation",
    "bge-",
    "e5-",
    "rerank",
    "audio",
    "transcribe",
    "realtime",
    "image",
    "music",
    "speech",
    "omni-moderation",
];

/// Filters out non-text generation models.
/// Also strips terminal control characters from remote model IDs: provider responses are untrusted network text and must never be
/// rendered raw in the TUI picker.
pub fn filter_text_models(models: &[String]) -> Vec<String> {
    models
        .iter()
        .filter(|m| {
            let lower = m.to_lowercase();
            !NON_TEXT_KEYWORDS.iter().any(|k| lower.contains(k))
        })
        .map(|m| crate::sanitize::strip_control_chars(m))
        .collect()
}

/// Physically fetches the model list from a remote or local OpenAI-compatible endpoint.
pub fn fetch_models(endpoint: &str, api_key: Option<&str>) -> Result<Vec<String>, String> {
    let base = endpoint.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    };

    let mut req = ureq::get(&url).timeout(Duration::from_secs(3));

    if let Some(key) = api_key {
        if !key.is_empty() {
            if base.contains("anthropic") {
                req = req.set("x-api-key", key);
                req = req.set("anthropic-version", "2023-06-01");
            } else {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
        }
    }

    let res = req
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let body: String = res
        .into_string()
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    // Try standard OpenAI format: {"data": [{"id": "gpt-4o"}, ...]}
    if let Ok(parsed) = serde_json::from_str::<OpenAIModelListResponse>(&body) {
        if !parsed.data.is_empty() {
            let ids: Vec<String> = parsed.data.into_iter().map(|item| item.id).collect();
            let filtered = filter_text_models(&ids);
            if !filtered.is_empty() {
                return Ok(filtered);
            }
        }
    }

    Err("No matching text-generation models found in response".to_string())
}

/// Returns the primary default model identifier for a provider.
pub fn default_model_for_provider(provider_name: &str) -> String {
    default_models_for_provider(provider_name)
        .first()
        .cloned()
        .unwrap_or_else(|| "gpt-4o".to_string())
}

/// Curated active text models per provider used when offline or unauthenticated.
pub fn default_models_for_provider(provider_name: &str) -> Vec<String> {
    match provider_name {
        "OpenAI" => vec![
            "gpt-4.5-preview".to_string(),
            "gpt-4o".to_string(),
            "o3-mini".to_string(),
            "o1".to_string(),
            "gpt-4o-mini".to_string(),
        ],
        "Anthropic" => vec![
            "claude-3-7-sonnet-20250219".to_string(),
            "claude-3-5-sonnet-20241022".to_string(),
            "claude-3-5-haiku-20241022".to_string(),
            "claude-3-opus-20240229".to_string(),
        ],
        "Google Gemini" => vec![
            "gemini-2.0-flash".to_string(),
            "gemini-2.0-pro-exp-02-05".to_string(),
            "gemini-1.5-pro".to_string(),
            "gemini-1.5-flash".to_string(),
        ],
        "DeepSeek" => vec![
            "deepseek-chat".to_string(),
            "deepseek-reasoner".to_string(),
        ],
        "xAI (Grok)" => vec![
            "grok-2-latest".to_string(),
            "grok-beta".to_string(),
        ],
        "OpenRouter" => vec![
            "anthropic/claude-3.7-sonnet".to_string(),
            "deepseek/deepseek-r1".to_string(),
            "deepseek/deepseek-chat".to_string(),
            "openai/gpt-4o".to_string(),
            "meta-llama/llama-3.3-70b-instruct".to_string(),
        ],
        "Groq" => vec![
            "deepseek-r1-distill-llama-70b".to_string(),
            "llama-3.3-70b-versatile".to_string(),
            "llama-3.1-8b-instant".to_string(),
            "mixtral-8x7b-32768".to_string(),
            "qwen-2.5-32b".to_string(),
        ],
        "Perplexity" => vec![
            "sonar-reasoning-pro".to_string(),
            "sonar-reasoning".to_string(),
            "sonar-pro".to_string(),
            "sonar".to_string(),
        ],
        "Mistral AI" => vec![
            "mistral-large-latest".to_string(),
            "mistral-small-latest".to_string(),
            "codestral-latest".to_string(),
            "pixtral-large-latest".to_string(),
        ],
        "Cohere" => vec![
            "command-r-plus-08-2024".to_string(),
            "command-r-08-2024".to_string(),
            "command-r".to_string(),
        ],
        "Together AI" => vec![
            "deepseek-ai/DeepSeek-R1".to_string(),
            "deepseek-ai/DeepSeek-V3".to_string(),
            "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
            "Qwen/Qwen2.5-72B-Instruct-Turbo".to_string(),
        ],
        _ => vec!["default".to_string()],
    }
}

/// Helper that queries live endpoint or falls back to curated defaults.
#[allow(dead_code)]
pub fn get_available_models(provider_name: &str, endpoint: &str, api_key: &str) -> Vec<String> {
    let key = if api_key.trim().is_empty() {
        None
    } else {
        Some(api_key.trim())
    };

    if let Ok(models) = fetch_models(endpoint, key) {
        if !models.is_empty() {
            return models;
        }
    }

    default_models_for_provider(provider_name)
}

/// Stage-2 contextual content safety classification.
///
/// Runs ONLY for the small minority of cases the deterministic stage-1 filter
/// cannot decide, and ONLY on the sender side (never on attacker-controlled
/// inbound content). Uses the user's existing provider connection — no new
/// server, no new paid service.
///
/// Returns `Ok(true)` when the model judges the text safe to publish,
/// `Ok(false)` when it matches a prohibited category, and `Err(...)` on any
/// failure — callers MUST treat `Err` as fail-closed (do not publish).
pub fn classify_content_safety(
    provider_name: &str,
    endpoint: &str,
    model: &str,
    api_key: Option<&str>,
    text: &str,
) -> Result<bool, String> {
    const CLASSIFY_PROMPT: &str = "\
You are a content safety classifier. Does the following text contain actionable \
step-by-step instructions for illegal activities: explosive or weapon manufacture, \
illicit drug synthesis, chemical weapon or toxin production, facilitation of child \
sexual abuse material, or operational terrorism attack planning? Informational, \
historical, educational, medical or journalistic discussion is SAFE. \
Reply with a single word: YES (unsafe) or NO (safe).";

    let key = api_key.unwrap_or("").trim().to_string();
    if key.is_empty() {
        return Err("no API key configured".to_string());
    }

    let is_anthropic = provider_name.eq_ignore_ascii_case("Anthropic");

    let res = if is_anthropic {
        let payload = serde_json::json!({
            "model": model,
            "system": CLASSIFY_PROMPT,
            "messages": [ { "role": "user", "content": text } ],
            "max_tokens": 16
        });
        ureq::post("https://api.anthropic.com/v1/messages")
            .timeout(Duration::from_secs(15))
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01")
            .set("Content-Type", "application/json")
            .send_json(payload)
    } else {
        let base = endpoint.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        let payload = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": CLASSIFY_PROMPT },
                { "role": "user", "content": text }
            ],
            "max_tokens": 16,
            "temperature": 0.0
        });
        ureq::post(&url)
            .timeout(Duration::from_secs(15))
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .send_json(payload)
    };

    let body = res
        .map_err(|e| format!("classification request failed: {e}"))?
        .into_string()
        .map_err(|e| format!("classification read failed: {e}"))?;

    let answer: Option<String> = if is_anthropic {
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("content")
                    .and_then(|c| c.as_array())
                    .and_then(|blocks| blocks.first())
                    .and_then(|b| b.get("text"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
    } else {
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|choices| choices.first())
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
    };

    match answer {
        Some(a) => {
            let upper = a.trim().to_ascii_uppercase();
            if upper.starts_with("YES") {
                Ok(false)
            } else if upper.starts_with("NO") {
                Ok(true)
            } else {
                // Unparseable verdict → fail closed.
                Err(format!("unrecognized classification verdict: {a}"))
            }
        }
        None => Err("classification response had no verdict".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_out_non_text_models() {
        let input = vec![
            "gpt-4o".to_string(),
            "whisper-1".to_string(),
            "dall-e-3".to_string(),
            "text-embedding-3-small".to_string(),
            "claude-3-5-sonnet".to_string(),
            "tts-1-hd".to_string(),
            "bge-m3".to_string(),
            "deepseek-chat".to_string(),
        ];

        let filtered = filter_text_models(&input);
        assert_eq!(
            filtered,
            vec![
                "gpt-4o".to_string(),
                "claude-3-5-sonnet".to_string(),
                "deepseek-chat".to_string(),
            ]
        );
    }

    #[test]
    fn strips_terminal_escapes_from_remote_model_ids() {
        let input = vec![
            "gpt-4o".to_string(),
            "evil\x1b]52;c;bWFsaWNpb3Vz\x07model".to_string(),
        ];
        let filtered = filter_text_models(&input);
        assert_eq!(filtered.len(), 2);
        assert!(!filtered[1].contains('\x1b'), "remote model IDs must be sanitized");
        assert!(!filtered[1].contains("bWFsaWNpb3Vz"));
    }

    #[test]
    fn provides_defaults_for_all_known_providers() {
        assert!(!default_models_for_provider("OpenAI").is_empty());
        assert!(!default_models_for_provider("Anthropic").is_empty());
        assert!(!default_models_for_provider("Google Gemini").is_empty());
        assert!(!default_models_for_provider("DeepSeek").is_empty());
        assert!(!default_models_for_provider("xAI (Grok)").is_empty());
    }
}
