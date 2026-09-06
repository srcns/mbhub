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
        ensure_dir_owner_only(&dir);
        return dir.join(".env");
    }
    PathBuf::from(".env")
}

/// Best-effort owner-only (0700) permission enforcement on the data
/// directory that holds the `.env` secret file (Unix only).
#[cfg(unix)]
fn ensure_dir_owner_only(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(dir) {
        if meta.is_dir() && meta.permissions().mode() & 0o777 != 0o700 {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
}

#[cfg(not(unix))]
fn ensure_dir_owner_only(_dir: &std::path::Path) {}

/// Best-effort owner-only (0600) permission enforcement on Unix.
#[cfg(unix)]
fn ensure_owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn ensure_owner_only(_path: &std::path::Path) {}

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

    // Repair drifted permissions before the contents are used: the file holds
    // plaintext API keys and must stay owner-only (0600) on Unix.
    ensure_owner_only(&path);

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
///
/// The write goes through a unique same-directory temp file that is created
/// with 0600 permissions on Unix and then renamed over the destination, so
/// the file is never briefly world-readable (no write-then-chmod window) and
/// readers never observe a partially written secret store. On any failure the
/// previous file is left untouched (fail closed) — there is deliberately no
/// insecure direct-write fallback.
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

    let _ = write_file_atomically_owner_only(&path, content.as_bytes());
}

/// Atomically replaces `path` with `bytes` via a unique same-directory temp
/// file (created with owner-only 0600 permissions on Unix) followed by a
/// rename. `rename` replaces the destination atomically and does not follow a
/// symlink placed at the destination path.
fn write_file_atomically_owner_only(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let _ = std::fs::create_dir_all(parent);
    let base = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mbhub_env")
        .to_string();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // PID + timestamp + attempt counter make the temp name effectively unique,
    // so a stale or pre-planted temp file can never be clobbered.
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..4u32 {
        let tmp = parent.join(format!(".{base}.{}.{}.{}.tmp", std::process::id(), nanos, attempt));
        match write_via_temp(&tmp, bytes) {
            Ok(()) => {
                return match std::fs::rename(&tmp, path) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        Err(e)
                    }
                };
            }
            // Temp name collision (should not happen): retry with a new name.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::other("could not create a unique temporary .env file")
    }))
}

/// Creates the temp file exclusively (0600 on Unix — no world-readable window
/// before the chmod) and writes the payload into it.
#[cfg(unix)]
fn write_via_temp(tmp: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_via_temp(tmp: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    f.write_all(bytes)
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

    #[test]
    fn saved_env_file_is_owner_only_and_leaves_no_temp_files() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_path = PathBuf::from("mbhub_test_env_perm.env");
        let _ = std::fs::remove_file(&temp_path);
        unsafe {
            std::env::set_var("MBHUB_ENV_FILE", &temp_path);
        }

        save_env_var("TEST_PERM_KEY", "secret-value");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&temp_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "fresh .env must be created owner-only");
        }

        // Updating through the atomic path keeps the file owner-only and
        // leaves no temp siblings behind.
        save_env_var("TEST_PERM_KEY", "rotated-value");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&temp_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "updated .env must stay owner-only");
        }
        let dir = temp_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with(".mbhub_test_env_perm.env.")
            })
            .collect();
        assert!(leftovers.is_empty(), "atomic write left temp files: {leftovers:?}");

        let loaded = load_env_file();
        assert_eq!(loaded.get("TEST_PERM_KEY").unwrap(), "rotated-value");

        let _ = std::fs::remove_file(temp_path);
        unsafe {
            std::env::remove_var("MBHUB_ENV_FILE");
        }
    }

    #[cfg(unix)]
    #[test]
    fn load_repairs_loose_env_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_path = PathBuf::from("mbhub_test_env_repair.env");
        let _ = std::fs::remove_file(&temp_path);
        std::fs::write(&temp_path, "TEST_REPAIR_KEY=value\n").unwrap();
        // Simulate a drifted (world-readable) secret store.
        std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        unsafe {
            std::env::set_var("MBHUB_ENV_FILE", &temp_path);
        }

        let loaded = load_env_file();
        assert_eq!(loaded.get("TEST_REPAIR_KEY").unwrap(), "value");
        let mode = std::fs::metadata(&temp_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "load must repair drifted permissions to 0600");

        let _ = std::fs::remove_file(temp_path);
        unsafe {
            std::env::remove_var("MBHUB_ENV_FILE");
        }
    }
}
