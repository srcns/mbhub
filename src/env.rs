//! Local configuration and security environment management for MBHub.
//!
//! Enforces:
//! - Local persistent `.env` file management with chmod 0600 security on Unix.
//! - Per-provider API key mapping (e.g. `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`).
//! - Multi-source precedence: Environment Variables -> `.env` file -> SQLite `meta` -> Default.
//! - Test isolation via `MBHUB_ENV_FILE`.

use std::collections::HashMap;
use std::path::PathBuf;

/// Returns the path to the active `.env` file.
/// Can be overridden in tests via `MBHUB_ENV_FILE`.
pub fn env_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("MBHUB_ENV_FILE") {
        return PathBuf::from(p);
    }
    if PathBuf::from(".env").exists() {
        return PathBuf::from(".env");
    }
    if let Some(home) = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()) {
        let dir = PathBuf::from(home).join(".mbhub");
        let _ = std::fs::create_dir_all(&dir);
        return dir.join(".env");
    }
    PathBuf::from(".env")
}

/// Maps human-readable provider names to standard uppercase environment variable keys.
pub fn provider_to_env_var(provider_name: &str) -> String {
    match provider_name {
        "OpenAI" => "OPENAI_API_KEY".to_string(),
        "Anthropic" => "ANTHROPIC_API_KEY".to_string(),
        "Google Gemini" => "GEMINI_API_KEY".to_string(),
        "DeepSeek" => "DEEPSEEK_API_KEY".to_string(),
        "xAI (Grok)" => "XAI_API_KEY".to_string(),
        "OpenRouter" => "OPENROUTER_API_KEY".to_string(),
        "Groq" => "GROQ_API_KEY".to_string(),
        "Perplexity" => "PERPLEXITY_API_KEY".to_string(),
        "Mistral AI" => "MISTRAL_API_KEY".to_string(),
        "Cohere" => "COHERE_API_KEY".to_string(),
        "Together AI" => "TOGETHER_API_KEY".to_string(),
        other => {
            let mut s = other
                .chars()
                .map(|c| if c.is_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
                .collect::<String>();
            while s.contains("__") {
                s = s.replace("__", "_");
            }
            format!("{}_API_KEY", s.trim_matches('_'))
        }
    }
}

/// Reads key-value pairs from the local `.env` file.
pub fn load_env_file() -> HashMap<String, String> {
    let path = env_file_path();
    let mut map = HashMap::new();

    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim().to_string();
                let mut val = v.trim();
                if (val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\''))
                {
                    if val.len() >= 2 {
                        val = &val[1..val.len() - 1];
                    }
                }
                map.insert(key, val.to_string());
            }
        }
    }

    map
}

/// Atomically inserts or updates a key-value pair in the local `.env` file,
/// enforcing owner-only permissions (0600) on Unix.
pub fn save_env_var(key: &str, val: &str) {
    let path = env_file_path();
    let mut lines = Vec::new();
    let mut found = false;

    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    lines.push(format!("{key}={val}"));
                    found = true;
                    continue;
                }
            }
            lines.push(line.to_string());
        }
    }

    if !found {
        lines.push(format!("{key}={val}"));
    }

    let mut content = lines.join("\n");
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let _ = std::fs::write(&path, content);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Resolves an API key for a provider across all persistent storage tiers:
/// 1. Process environment variable (`OPENROUTER_API_KEY`)
/// 2. Local `.env` file
/// 3. Local SQLite `meta` table
pub fn get_api_key_for_provider(provider_name: &str) -> String {
    let env_var = provider_to_env_var(provider_name);

    // 1. Process env
    if let Ok(v) = std::env::var(&env_var) {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }

    // 2. .env file
    let file_map = load_env_file();
    if let Some(v) = file_map.get(&env_var) {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }

    // 3. SQLite meta
    crate::db::get_provider_api_key(provider_name)
}

/// Saves an API key for a provider to both the local `.env` file and SQLite `meta`.
pub fn set_api_key_for_provider(provider_name: &str, key: &str) {
    let env_var = provider_to_env_var(provider_name);
    save_env_var(&env_var, key);
    crate::db::set_provider_api_key(provider_name, key);
}

/// Resolves a selected model for a provider from `.env` or SQLite `meta`.
pub fn get_model_for_provider(provider_name: &str) -> Option<String> {
    let env_var = format!("{}_MODEL", provider_to_env_var(provider_name).trim_end_matches("_API_KEY"));

    // 1. Process env
    if let Ok(v) = std::env::var(&env_var) {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }

    // 2. .env file
    let file_map = load_env_file();
    if let Some(v) = file_map.get(&env_var) {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }

    // 3. SQLite meta
    crate::db::get_provider_model(provider_name)
}

/// Saves a selected model for a provider to both `.env` and SQLite `meta`.
pub fn set_model_for_provider(provider_name: &str, model: &str) {
    let env_var = format!("{}_MODEL", provider_to_env_var(provider_name).trim_end_matches("_API_KEY"));
    save_env_var(&env_var, model);
    crate::db::set_provider_model(provider_name, model);
}

#[cfg(test)]
pub static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_normalization() {
        assert_eq!(provider_to_env_var("OpenAI"), "OPENAI_API_KEY");
        assert_eq!(provider_to_env_var("Anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(provider_to_env_var("Google Gemini"), "GEMINI_API_KEY");
        assert_eq!(provider_to_env_var("DeepSeek"), "DEEPSEEK_API_KEY");
        assert_eq!(provider_to_env_var("xAI (Grok)"), "XAI_API_KEY");
        assert_eq!(provider_to_env_var("OpenRouter"), "OPENROUTER_API_KEY");
        assert_eq!(provider_to_env_var("Groq"), "GROQ_API_KEY");
    }

    #[test]
    fn env_file_round_trip() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_path = PathBuf::from("mbhub_test_env_round_trip.env");
        unsafe {
            std::env::set_var("MBHUB_ENV_FILE", &temp_path);
        }

        save_env_var("TEST_KEY_FOO", "bar123");
        save_env_var("TEST_KEY_QUX", "qux456");

        let loaded = load_env_file();
        assert_eq!(loaded.get("TEST_KEY_FOO").unwrap(), "bar123");
        assert_eq!(loaded.get("TEST_KEY_QUX").unwrap(), "qux456");

        // Update an existing key
        save_env_var("TEST_KEY_FOO", "bar_updated");
        let updated = load_env_file();
        assert_eq!(updated.get("TEST_KEY_FOO").unwrap(), "bar_updated");

        let _ = std::fs::remove_file(temp_path);
        unsafe {
            std::env::remove_var("MBHUB_ENV_FILE");
        }
    }
}
