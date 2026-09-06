//! Friction-Free Auto-Update Engine for MBHub.
//!
//! Resolution strategy:
//! 1. **mbhub.dev manifest** (`https://mbhub.dev/releases/latest.txt`) — the
//!    public, auth-free distribution channel used by the installers. Works
//!    even while the GitHub repository is private.
//! 2. **GitHub API fallback** — for public forks or future open-sourcing.
//!
//! The downloaded asset is verified against the release's `SHA256SUMS.txt`,
//! the executable is extracted from `.tar.gz` / `.zip` archives, sanity-checked,
//! and then replaced atomically:
//! - Strictly updates ONLY the executable binary (`std::env::current_exe()`).
//! - NEVER modifies, moves, or deletes `~/.mbhub/mbhub.db` or `.env` configuration.
//! - Checks semantic version numbers against `env!("CARGO_PKG_VERSION")`.
//! - Supports check-only mode and full atomic upgrade.

use std::fs;
use std::io::{Cursor, Read};
use std::time::Duration;
use serde::Deserialize;

const GITHUB_REPO: &str = "srcns/mbhub";
const WEBSITE_MANIFEST_URL: &str = "https://mbhub.dev/releases/latest.txt";

/// Hard cap on downloaded release payloads (archives + overhead).
const MAX_DOWNLOAD_BYTES: u64 = 100_000_000;

/// Hard cap on the *decompressed* size of a single archive entry — a
/// decompression-bomb guard. A tiny `.tar.gz`/`.zip` can expand to gigabytes,
/// so the extracted entry is capped independently of the compressed download
/// cap (256 MiB is far above any legitimate mbhub binary).
const MAX_EXTRACTED_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Deserialize, Debug)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    #[allow(dead_code)]
    size: usize,
}

#[derive(Deserialize, Debug)]
struct GitHubRelease {
    tag_name: String,
    #[allow(dead_code)]
    name: Option<String>,
    #[allow(dead_code)]
    published_at: Option<String>,
    assets: Vec<ReleaseAsset>,
}

/// Where the latest release metadata came from, and therefore where the
/// binaries are downloaded from.
enum UpdateSource {
    Website(String),
    GitHub(GitHubRelease),
}

/// Release artifact base name for the running platform (without extension).
fn platform_artifact_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("mbhub-linux-x64"),
        ("linux", "aarch64") => Some("mbhub-linux-arm64"),
        ("macos", "aarch64") => Some("mbhub-macos-arm64"),
        ("macos", "x86_64") => Some("mbhub-macos-x64"),
        ("windows", "x86_64") => Some("mbhub-windows-x64"),
        _ => None,
    }
}

/// True when a release asset carries the executable for `base` (matching
/// archive, raw binary, and Windows `.exe` spellings).
fn asset_matches(name: &str, base: &str) -> bool {
    name == base
        || name.strip_suffix(".tar.gz") == Some(base)
        || name.strip_suffix(".zip") == Some(base)
        || name.strip_suffix(".exe") == Some(base)
}

/// Accepts strictly versioned tags: `v` followed by dot-separated integers.
fn valid_tag(tag: &str) -> bool {
    let Some(body) = tag.strip_prefix('v') else {
        return false;
    };
    !body.is_empty()
        && body
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn download_bytes(url: &str, user_agent: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let res = ureq::get(url)
        .set("User-Agent", user_agent)
        .timeout(timeout)
        .call()
        .map_err(|e| format!("Failed to download {url}: {e}"))?;

    let mut buf = Vec::new();
    let mut limited = res.into_reader().take(MAX_DOWNLOAD_BYTES);
    std::io::copy(&mut limited, &mut buf).map_err(|e| format!("Failed to read {url}: {e}"))?;
    if buf.len() as u64 >= MAX_DOWNLOAD_BYTES {
        return Err(format!("Downloaded release file exceeds the {MAX_DOWNLOAD_BYTES}-byte safety cap."));
    }
    if buf.is_empty() {
        return Err(format!("Downloaded release file is empty: {url}"));
    }
    Ok(buf)
}

/// Resolves the latest release: website manifest first, GitHub API fallback.
fn resolve_latest_release(user_agent: &str) -> Result<(UpdateSource, String, String), String> {
    // 1. Public website manifest (works for private GitHub repositories too).
    match ureq::get(WEBSITE_MANIFEST_URL)
        .set("User-Agent", user_agent)
        .timeout(Duration::from_secs(15))
        .call()
    {
        Ok(res) => {
            if let Ok(body) = res.into_string() {
                let tag = body.trim().to_string();
                if valid_tag(&tag) {
                    return Ok((
                        UpdateSource::Website(tag.clone()),
                        tag.clone(),
                        tag.trim_start_matches('v').to_string(),
                    ));
                }
            }
        }
        Err(_) => {}
    }

    // 2. GitHub API fallback.
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let res = ureq::get(&url)
        .set("User-Agent", user_agent)
        .set("Accept", "application/vnd.github.v3+json")
        .timeout(Duration::from_secs(15))
        .call();

    let release: GitHubRelease = match res {
        Ok(r) => r.into_json().map_err(|e| format!("Failed to parse release metadata: {e}"))?,
        Err(ureq::Error::Status(404, _)) => {
            return Err(format!(
                "No release information found at {WEBSITE_MANIFEST_URL} or on GitHub repo '{GITHUB_REPO}'."
            ));
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("GitHub API returned HTTP {code}. Please try again later."));
        }
        Err(e) => {
            return Err(format!("Failed to connect to GitHub: {e}"));
        }
    };

    let latest_tag = release.tag_name.trim().to_string();
    let latest_version = latest_tag.trim_start_matches('v').to_string();
    Ok((UpdateSource::GitHub(release), latest_tag, latest_version))
}

/// Extracts the executable bytes from a release asset payload. Raw binary
/// assets pass through untouched; `.tar.gz` and `.zip` archives are unpacked
/// in memory (both formats are produced by the release CI pipeline).
///
/// The decompressed entry size is capped (`MAX_EXTRACTED_ENTRY_BYTES`) so a
/// malicious archive cannot exhaust memory with a decompression bomb.
fn extract_binary(asset_name: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    extract_binary_capped(asset_name, bytes, MAX_EXTRACTED_ENTRY_BYTES)
}

/// Internal variant with an injectable entry cap (used directly by tests).
fn extract_binary_capped(
    asset_name: &str,
    bytes: &[u8],
    max_entry_bytes: u64,
) -> Result<Vec<u8>, String> {
    if asset_name.ends_with(".tar.gz") {
        let decoder = flate2::read::GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|e| format!("invalid tar.gz archive: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("invalid tar.gz entry: {e}"))?;
            let path = entry
                .path()
                .map_err(|e| format!("invalid tar.gz entry path: {e}"))?;
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname == "mbhub" || fname == "mbhub.exe" {
                // `take` bounds the decompressed read: the entry can never
                // expand past the cap in memory.
                let mut buf = Vec::new();
                let mut limited = entry.take(max_entry_bytes);
                limited
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("failed to read binary from archive: {e}"))?;
                if buf.len() as u64 >= max_entry_bytes {
                    return Err(format!(
                        "archive entry expands to the {max_entry_bytes}-byte extraction cap — refusing possible decompression bomb"
                    ));
                }
                return Ok(buf);
            }
        }
        Err("archive does not contain the mbhub binary".to_string())
    } else if asset_name.ends_with(".zip") {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("invalid zip archive: {e}"))?;
        for idx in 0..archive.len() {
            let entry = archive
                .by_index(idx)
                .map_err(|e| format!("invalid zip entry: {e}"))?;
            let fname = entry.name().rsplit('/').next().unwrap_or("");
            if fname == "mbhub" || fname == "mbhub.exe" {
                let mut buf = Vec::new();
                let mut limited = entry.take(max_entry_bytes);
                limited
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("failed to read binary from archive: {e}"))?;
                if buf.len() as u64 >= max_entry_bytes {
                    return Err(format!(
                        "zip entry expands to the {max_entry_bytes}-byte extraction cap — refusing possible decompression bomb"
                    ));
                }
                return Ok(buf);
            }
        }
        Err("zip archive does not contain the mbhub binary".to_string())
    } else {
        Ok(bytes.to_vec())
    }
}

/// Verifies the downloaded asset against the release's SHA256SUMS.txt.
///
/// Verification is mandatory: a release that does not publish a checksum
/// manifest (`sums_url == None`) is rejected outright — there is no
/// "skipped" path, so an unverified binary can never be installed. The
/// manifest download goes through `fetch` so tests can inject a fake one
/// without network access.
fn verify_release_asset(
    asset_name: &str,
    asset_bytes: &[u8],
    sums_url: Option<&str>,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let Some(sums_url) = sums_url else {
        return Err(format!(
            "Release does not publish a SHA256SUMS.txt checksum manifest — refusing to install an unverified binary. \
             Please update manually from https://github.com/{GITHUB_REPO}/releases"
        ));
    };
    let sums_bytes = fetch(sums_url)?;
    let sums_text = String::from_utf8_lossy(&sums_bytes).to_string();
    verify_checksum(asset_name, asset_bytes, &sums_text)?;
    Ok(())
}

/// Verifies the SHA-256 of `bytes` against the release's SHA256SUMS.txt.
fn verify_checksum(asset_name: &str, bytes: &[u8], sums_text: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let mut expected: Option<String> = None;
    for line in sums_text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(hex), Some(name)) = (parts.next(), parts.next()) {
            if name == asset_name {
                expected = Some(hex.to_lowercase());
                break;
            }
        }
    }

    let Some(expected) = expected else {
        return Err(format!("SHA256SUMS.txt has no entry for {asset_name}"));
    };

    let digest = Sha256::digest(bytes);
    let actual = format!("{digest:x}");
    if actual == expected {
        Ok(())
    } else {
        Err("SHA-256 checksum mismatch — downloaded file does not match the release manifest".to_string())
    }
}

/// Cheap sanity check that the extracted payload is a real executable for
/// this platform, not an HTML error page or a truncated archive.
fn looks_like_executable(bytes: &[u8]) -> bool {
    #[cfg(target_os = "windows")]
    {
        bytes.starts_with(b"MZ")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        bytes.starts_with(b"\x7fELF")
    }
    #[cfg(target_os = "macos")]
    {
        // Mach-O magic: 32-bit and 64-bit, both endiannesses.
        bytes.starts_with(b"\xfe\xed\xfa\xce")
            || bytes.starts_with(b"\xce\xfa\xed\xfe")
            || bytes.starts_with(b"\xfe\xed\xfa\xcf")
            || bytes.starts_with(b"\xcf\xfa\xed\xfe")
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        !bytes.is_empty()
    }
}

/// Checks the latest release and optionally executes an atomic binary upgrade.
#[cfg_attr(feature = "publisher", allow(unreachable_code))]
pub fn execute_update(check_only: bool) -> Result<(), String> {
    // Maintainer `publisher` builds must never be replaced by the public
    // distribution binaries downloaded from the release channel — they carry
    // capabilities that are intentionally excluded from distributed builds.
    #[cfg(feature = "publisher")]
    {
        let _ = check_only;
        println!("Publisher build detected: automatic updates are disabled for this profile.");
        println!("Rebuild from source with `cargo build --release --features publisher` instead.");
        return Ok(());
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let user_agent = format!("mbhub-updater/{current_version}");
    let current_exe = std::env::current_exe().map_err(|e| format!("Cannot locate current executable: {e}"))?;

    eprintln!("Checking for latest MBHub release...");

    let (source, latest_tag, latest_version) = resolve_latest_release(&user_agent)?;

    let has_newer = is_version_newer(current_version, &latest_version);

    if !has_newer {
        println!("✓ MBHub is up to date (v{current_version}).");
        return Ok(());
    }

    println!("Found new version: v{latest_version} (installed: v{current_version})");

    if check_only {
        println!("Run `mbhub update` to upgrade in seconds.");
        return Ok(());
    }

    let Some(base_name) = platform_artifact_name() else {
        return Err(format!(
            "No release binary available for {}-{}.\nPlease download manually from: https://github.com/{GITHUB_REPO}/releases/tag/{latest_tag}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    };

    // Determine the asset name, its URL, and the checksum manifest URL.
    let (asset_name, asset_url, sums_url) = match &source {
        UpdateSource::Website(tag) => {
            let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
            let name = format!("{base_name}.{ext}");
            let url = format!("https://mbhub.dev/releases/{tag}/{name}");
            let sums = format!("https://mbhub.dev/releases/{tag}/SHA256SUMS.txt");
            (name, url, Some(sums))
        }
        UpdateSource::GitHub(release) => {
            let asset = release
                .assets
                .iter()
                .find(|a| asset_matches(&a.name, base_name))
                .ok_or_else(|| {
                    format!(
                        "No release binary found for {} in release {latest_tag}.\nPlease download manually from: https://github.com/{GITHUB_REPO}/releases/tag/{latest_tag}",
                        base_name
                    )
                })?;
            let sums = release
                .assets
                .iter()
                .find(|a| a.name == "SHA256SUMS.txt")
                .map(|a| a.browser_download_url.clone());
            (
                asset.name.clone(),
                asset.browser_download_url.clone(),
                sums,
            )
        }
    };

    println!("Downloading {asset_name}...");
    let asset_bytes = download_bytes(&asset_url, &user_agent, Duration::from_secs(120))?;

    // Cryptographic verification against the release's SHA256SUMS.txt.
    // Verification is MANDATORY: a release without a checksum manifest is
    // refused outright — an unverified binary is never installed.
    let mut fetch_sums =
        |url: &str| download_bytes(url, &user_agent, Duration::from_secs(60));
    verify_release_asset(&asset_name, &asset_bytes, sums_url.as_deref(), &mut fetch_sums)?;
    println!("SHA-256 checksum verified against the release manifest.");

    // Unpack the executable from the archive (or use the raw binary as-is).
    let binary_bytes = extract_binary(&asset_name, &asset_bytes)?;
    if !looks_like_executable(&binary_bytes) {
        return Err("Extracted payload does not look like an executable — aborting update.".to_string());
    }

    println!("Applying atomic executable update to: {}", current_exe.display());

    // Atomic replacement of current_exe
    replace_executable_atomically(&current_exe, &binary_bytes)?;

    println!();
    println!("═════════════════════════════════════════════════════════════════");
    println!("✓ Successfully updated MBHub to v{latest_version}!");
    println!("✓ Your local database (~/.mbhub/mbhub.db) remains 100% intact.");
    println!("═════════════════════════════════════════════════════════════════");

    // If daemon service is running, restart it
    if crate::ipc::try_query_daemon(&crate::ipc::IpcRequest::Ping).is_some() {
        println!("Note: Restarting background service...");
        let _ = crate::service::stop();
        let _ = crate::service::start();
    }

    Ok(())
}

/// Compares two semver strings (e.g. "1.0.0" vs "1.0.1").
fn is_version_newer(current: &str, candidate: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .filter_map(|part| part.split('-').next()?.parse::<u64>().ok())
            .collect()
    };
    let c = parse(current);
    let cand = parse(candidate);
    cand > c
}

/// Atomically replaces the target executable file without touching user data.
fn replace_executable_atomically(exe_path: &std::path::Path, new_bytes: &[u8]) -> Result<(), String> {
    let parent = exe_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp_path = parent.join(format!(".mbhub_update_{}.tmp", std::process::id()));

    fs::write(&tmp_path, new_bytes)
        .map_err(|e| format!("Failed to write temporary binary at {}: {e}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755));
    }

    #[cfg(unix)]
    {
        fs::rename(&tmp_path, exe_path)
            .map_err(|e| format!("Atomic rename failed: {e}. You may need elevated permissions (sudo)."))?;
    }

    #[cfg(windows)]
    {
        let old_path = parent.join(format!(".mbhub_old_{}.exe", std::process::id()));
        let _ = fs::rename(exe_path, &old_path);
        fs::rename(&tmp_path, exe_path)
            .map_err(|e| format!("Failed to replace Windows executable: {e}"))?;
        let _ = fs::remove_file(old_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    #[test]
    fn semver_comparison_detects_newer_versions() {
        assert!(is_version_newer("1.0.0", "1.0.1"));
        assert!(is_version_newer("1.0.0", "1.1.0"));
        assert!(is_version_newer("1.0.0", "2.0.0"));
        assert!(!is_version_newer("1.0.1", "1.0.0"));
        assert!(!is_version_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn version_tag_validation() {
        assert!(valid_tag("v1.0.1"));
        assert!(valid_tag("v10.2.0"));
        assert!(!valid_tag("v1.0.1\n"));
        assert!(!valid_tag("1.0.1"));
        assert!(!valid_tag("v"));
        assert!(!valid_tag("v1.0-beta"));
        assert!(!valid_tag("v1..0"));
        assert!(!valid_tag("404: Not Found"));
    }

    #[test]
    fn asset_matching_covers_archives_raw_and_exe() {
        assert!(asset_matches("mbhub-linux-x64.tar.gz", "mbhub-linux-x64"));
        assert!(asset_matches("mbhub-macos-arm64.tar.gz", "mbhub-macos-arm64"));
        assert!(asset_matches("mbhub-windows-x64.zip", "mbhub-windows-x64"));
        assert!(asset_matches("mbhub-windows-x64", "mbhub-windows-x64"));
        assert!(asset_matches("mbhub-windows-x64.exe", "mbhub-windows-x64"));
        assert!(!asset_matches("mbhub-macos-x64.tar.gz", "mbhub-linux-x64"));
        assert!(!asset_matches("SHA256SUMS.txt", "mbhub-linux-x64"));
        assert!(!asset_matches("mbhub-linux-arm64.tar.gz", "mbhub-linux-x64"));
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn extracts_binary_from_tar_gz() {
        let payload = b"FAKE-MBHUB-BINARY-PAYLOAD".to_vec();
        let mut archive_bytes = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut archive_bytes, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "mbhub", payload.as_slice())
                .expect("appends file");
            let encoder = builder.into_inner().expect("finish tar");
            encoder.finish().expect("finish gz");
        }
        let extracted = extract_binary("mbhub-linux-x64.tar.gz", &archive_bytes).expect("extracts");
        assert_eq!(extracted, payload);
    }

    #[test]
    fn extracts_binary_from_zip() {
        let payload = b"FAKE-MBHUB-WINDOWS-BINARY".to_vec();
        let mut archive_bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut archive_bytes));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer
                .start_file("mbhub.exe", options)
                .expect("starts file");
            writer.write_all(&payload).expect("writes payload");
            writer.finish().expect("finishes archive");
        }
        let extracted = extract_binary("mbhub-windows-x64.zip", &archive_bytes).expect("extracts");
        assert_eq!(extracted, payload);
    }

    #[test]
    fn raw_asset_passes_through_unchanged() {
        let payload = b"RAW-BINARY".to_vec();
        let extracted = extract_binary("mbhub-linux-x64", &payload).expect("passes through");
        assert_eq!(extracted, payload);
    }

    #[test]
    fn checksum_verification_detects_tampering() {
        let bytes = b"release-bytes".to_vec();
        let sums = format!("{}  mbhub-linux-x64.tar.gz\n", sha256_hex(&bytes));
        assert!(verify_checksum("mbhub-linux-x64.tar.gz", &bytes, &sums).is_ok());

        let tampered = b"release-bytes-EVIL".to_vec();
        assert!(verify_checksum("mbhub-linux-x64.tar.gz", &tampered, &sums).is_err());
    }

    #[test]
    fn checksum_verification_requires_an_entry() {
        let sums = format!("{}  mbhub-windows-x64.zip\n", sha256_hex(b"other"));
        assert!(verify_checksum("mbhub-linux-x64.tar.gz", b"x", &sums).is_err());
    }

    #[test]
    fn update_rejected_without_checksum_manifest() {
        // A release without SHA256SUMS.txt must be refused — never installed
        // with a "verification skipped" warning.
        let result = verify_release_asset("mbhub-linux-x64.tar.gz", b"bytes", None, |_| {
            Ok(Vec::new())
        });
        let err = result.unwrap_err();
        assert!(err.contains("SHA256SUMS.txt"), "error should name the manifest: {err}");
        assert!(err.contains("refusing"), "error must state the refusal: {err}");
    }

    #[test]
    fn update_verifies_against_fetched_manifest() {
        let bytes = b"release-bytes".to_vec();
        let sums = format!("{}  mbhub-linux-x64.tar.gz\n", sha256_hex(&bytes));

        // Fake manifest "download" — no network involved.
        let mut fetch_ok = |url: &str| -> Result<Vec<u8>, String> {
            assert!(url.ends_with("SHA256SUMS.txt"));
            Ok(sums.clone().into_bytes())
        };
        assert!(verify_release_asset("mbhub-linux-x64.tar.gz", &bytes, Some("https://example/SHA256SUMS.txt"), &mut fetch_ok).is_ok());

        // A tampered payload must be rejected even when the manifest exists.
        let tampered = b"release-bytes-EVIL".to_vec();
        let mut fetch_ok2 = move |_url: &str| -> Result<Vec<u8>, String> {
            Ok(sums.clone().into_bytes())
        };
        assert!(verify_release_asset("mbhub-linux-x64.tar.gz", &tampered, Some("https://example/SHA256SUMS.txt"), &mut fetch_ok2).is_err());

        // A failing manifest download fails the update (fail closed).
        let mut fetch_fail = |_url: &str| -> Result<Vec<u8>, String> {
            Err("Failed to download manifest".to_string())
        };
        assert!(verify_release_asset("mbhub-linux-x64.tar.gz", &bytes, Some("https://example/SHA256SUMS.txt"), &mut fetch_fail).is_err());
    }

    fn build_tar_gz(payload: &[u8]) -> Vec<u8> {
        let mut archive_bytes = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut archive_bytes, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "mbhub", payload)
                .expect("appends file");
            let encoder = builder.into_inner().expect("finish tar");
            encoder.finish().expect("finish gz");
        }
        archive_bytes
    }

    fn build_zip(payload: &[u8]) -> Vec<u8> {
        let mut archive_bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut archive_bytes));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("mbhub.exe", options).expect("starts file");
            writer.write_all(payload).expect("writes payload");
            writer.finish().expect("finishes archive");
        }
        archive_bytes
    }

    #[test]
    fn tar_extraction_enforces_decompression_bomb_cap() {
        // 1 MiB of zeros compresses to a few KB — a classic bomb shape.
        let payload = vec![0u8; 1024 * 1024];
        let archive = build_tar_gz(&payload);

        // Below the cap the extraction is refused.
        let err = extract_binary_capped("mbhub-linux-x64.tar.gz", &archive, 64 * 1024).unwrap_err();
        assert!(err.contains("decompression bomb"), "unexpected error: {err}");

        // Above the cap the same archive extracts fine.
        let ok = extract_binary_capped("mbhub-linux-x64.tar.gz", &archive, 2 * 1024 * 1024).unwrap();
        assert_eq!(ok, payload);

        // The public entry point uses the 256 MiB production cap.
        assert_eq!(extract_binary("mbhub-linux-x64.tar.gz", &archive).unwrap(), payload);
    }

    #[test]
    fn zip_extraction_enforces_decompression_bomb_cap() {
        let payload = vec![0u8; 1024 * 1024];
        let archive = build_zip(&payload);

        let err = extract_binary_capped("mbhub-windows-x64.zip", &archive, 64 * 1024).unwrap_err();
        assert!(err.contains("decompression bomb"), "unexpected error: {err}");

        let ok = extract_binary_capped("mbhub-windows-x64.zip", &archive, 2 * 1024 * 1024).unwrap();
        assert_eq!(ok, payload);
    }

    #[test]
    fn executable_magic_detection() {
        #[cfg(all(unix, not(target_os = "macos")))]
        assert!(looks_like_executable(b"\x7fELF\x02\x01\x01"));
        #[cfg(target_os = "windows")]
        assert!(looks_like_executable(b"MZ\x90\x00"));
        #[cfg(target_os = "macos")]
        assert!(looks_like_executable(b"\xcf\xfa\xed\xfe"));
        assert!(!looks_like_executable(b"<!DOCTYPE html>"));
        assert!(!looks_like_executable(b"\x1f\x8b\x08gzip"));
    }
}
