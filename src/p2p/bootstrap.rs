//! Bootstrap peer sources for first contact.
//!
//! A fresh node needs at least one dialable multiaddr to join the swarm and
//! bootstrap its Kademlia routing table. Sources, in descending priority:
//!
//! 1. `MBHUB_BOOTSTRAP_PEERS` env (comma-separated multiaddrs, cap 16) —
//!    developer/operator escape hatch; when set it *replaces* the defaults.
//! 2. Embedded default list (compile-time; shipped with the binary).
//! 3. `https://mbhub.dev/bootstrap.json` (fetched fresh; result cached to
//!    `~/.mbhub/bootstrap-cache.json` for offline/failure resilience).
//! 4. The cache file alone (when the remote fetch fails).
//!
//! Security invariants: every source is parsed as a strict `Multiaddr`, all
//! lists are deduplicated and hard-capped, remote payloads are size-limited
//! and fetched over TLS with a short timeout. Bootstrap peers carry zero
//! authority — they are plain rendezvous addresses and every message they
//! relay passes the same content gates as any other peer.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use libp2p::Multiaddr;

/// Hard cap on bootstrap peers from any single source (Sybil list flooding).
pub const MAX_BOOTSTRAP_PEERS: usize = 16;

/// Public bootstrap manifest served from Cloudflare Pages ($0, static).
pub const BOOTSTRAP_URL: &str = "https://mbhub.dev/bootstrap.json";

/// Remote fetch timeout. First contact must not delay startup.
pub const BOOTSTRAP_FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// Remote payload ceiling (16 peers × ~100 bytes ≈ 2 KB; generous margin).
pub const MAX_BOOTSTRAP_PAYLOAD_BYTES: usize = 65_536;

/// How often the remote manifest is re-fetched by a running node.
pub const BOOTSTRAP_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Compile-time defaults. The Oracle bootstrap VMs are provisioned out of
/// band (see the rendezvous decision document); until their multiaddrs are
/// published this list is empty and the node relies on env / remote / cache
/// sources, mDNS for LAN peers, and logs a clear warning at startup.
pub const EMBEDDED_BOOTSTRAP_MULTIADDRS: &[&str] = &[
    // Placeholder entries are intentionally absent: an empty list is valid
    // and preferable to dead addresses.
];

/// Where the resolved bootstrap list came from (observability + tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapSource {
    /// `MBHUB_BOOTSTRAP_PEERS` env override.
    Env,
    /// Embedded compile-time defaults.
    Embedded,
    /// Remote manifest fetched successfully this run.
    Remote,
    /// Local cache from a previous successful fetch.
    Cache,
    /// No source yielded any address.
    None,
}

/// Result of resolving all sources.
#[derive(Clone, Debug)]
pub struct BootstrapList {
    pub addresses: Vec<Multiaddr>,
    pub source: BootstrapSource,
}

impl BootstrapList {
    fn empty() -> Self {
        Self {
            addresses: Vec::new(),
            source: BootstrapSource::None,
        }
    }
}

/// Path of the local bootstrap cache (`MBHUB_BOOTSTRAP_CACHE` override).
pub fn cache_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MBHUB_BOOTSTRAP_CACHE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())?;
    Some(PathBuf::from(home).join(".mbhub").join("bootstrap-cache.json"))
}

/// Parses a comma/newline separated multiaddr list, deduplicating and capping.
pub fn parse_multiaddr_list(raw: &str) -> Vec<Multiaddr> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in raw.split(|c: char| c == ',' || c == '\n' || c == '\r') {
        let s = part.trim();
        if s.is_empty() {
            continue;
        }
        if let Ok(addr) = s.parse::<Multiaddr>() {
            if seen.insert(addr.clone()) {
                out.push(addr);
                if out.len() >= MAX_BOOTSTRAP_PEERS {
                    break;
                }
            }
        }
    }
    out
}

/// Reads the env override (`MBHUB_BOOTSTRAP_PEERS`).
fn env_peers() -> Vec<Multiaddr> {
    match std::env::var("MBHUB_BOOTSTRAP_PEERS") {
        Ok(raw) => parse_multiaddr_list(&raw),
        Err(_) => Vec::new(),
    }
}

/// Embedded compile-time defaults.
fn embedded_peers() -> Vec<Multiaddr> {
    let raw = EMBEDDED_BOOTSTRAP_MULTIADDRS.join(",");
    parse_multiaddr_list(&raw)
}

/// Fetches and parses the remote manifest (`{"bootstraps": ["multiaddr"...]}`).
///
/// TLS-only (https), size-capped, short timeout; malformed entries are
/// skipped, not fatal. Returns `None` on any network or parse failure —
/// callers then fall back to the cache.
fn fetch_remote_peers() -> Option<Vec<Multiaddr>> {
    let response = ureq::get(BOOTSTRAP_URL)
        .timeout(BOOTSTRAP_FETCH_TIMEOUT)
        .call()
        .ok()?;

    // Size cap BEFORE reading: a hostile CDN response cannot balloon memory.
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BOOTSTRAP_PAYLOAD_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() > MAX_BOOTSTRAP_PAYLOAD_BYTES {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let list = parsed.get("bootstraps")?.as_array()?;
    let mut raws = String::new();
    for v in list {
        if let Some(s) = v.as_str() {
            raws.push_str(s);
            raws.push('\n');
        }
    }
    let peers = parse_multiaddr_list(&raws);
    if peers.is_empty() {
        None
    } else {
        Some(peers)
    }
}

/// Writes the cache file atomically with owner-only permissions.
fn write_cache(addresses: &[Multiaddr]) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let list: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
    let json = serde_json::json!({ "bootstraps": list, "saved_at": chrono::Local::now().to_rfc3339() });
    if let Ok(bytes) = serde_json::to_vec_pretty(&json) {
        let tmp = path.with_extension("tmp");
        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(&bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                }
                Ok(())
            })
            .is_ok()
        {
            let _ = std::fs::rename(&tmp, &path);
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Reads the cache file (empty on absence/corruption — the cache is best-effort).
fn read_cache() -> Vec<Multiaddr> {
    let Some(path) = cache_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
            let mut raws = String::new();
            if let Some(list) = parsed.get("bootstraps").and_then(|v| v.as_array()) {
                for v in list {
                    if let Some(s) = v.as_str() {
                        raws.push_str(s);
                        raws.push('\n');
                    }
                }
            }
            parse_multiaddr_list(&raws)
        }
        Err(_) => Vec::new(),
    }
}

/// Resolves the effective bootstrap list from all sources.
///
/// Precedence: env override replaces everything; otherwise embedded + remote
/// (or cache when the remote is unreachable) are merged, deduplicated and
/// capped. The remote fetch is attempted on every call; callers drive
/// periodic refresh with [`BOOTSTRAP_REFRESH_INTERVAL`].
pub fn resolve() -> BootstrapList {
    let env = env_peers();
    if !env.is_empty() {
        return BootstrapList {
            addresses: env,
            source: BootstrapSource::Env,
        };
    }

    let embedded = embedded_peers();
    let mut source = if embedded.is_empty() {
        BootstrapSource::None
    } else {
        BootstrapSource::Embedded
    };
    let mut merged = embedded;

    let remote = fetch_remote_peers();
    if let Some(remote) = remote.as_ref() {
        write_cache(remote);
    }

    let remote_or_cache = remote.unwrap_or_else(|| {
        let cache = read_cache();
        if !cache.is_empty() {
            source = BootstrapSource::Cache;
        }
        cache
    });
    if !remote_or_cache.is_empty() && source != BootstrapSource::Cache {
        source = BootstrapSource::Remote;
    }

    for addr in remote_or_cache {
        if merged.len() >= MAX_BOOTSTRAP_PEERS {
            break;
        }
        if !merged.contains(&addr) {
            merged.push(addr);
        }
    }

    if merged.is_empty() {
        BootstrapList::empty()
    } else {
        BootstrapList {
            addresses: merged,
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes env-mutating tests: `MBHUB_BOOTSTRAP_CACHE` is process-wide
    /// state and cargo runs tests in parallel threads.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parse_list_dedupes_and_caps() {
        let raw = "/ip4/127.0.0.1/tcp/1, /ip4/127.0.0.1/tcp/1\n/ip4/10.0.0.1/tcp/2,,garbage";
        let parsed = parse_multiaddr_list(raw);
        assert_eq!(parsed.len(), 2, "dedupe + skip invalid");
        assert_eq!(parsed[0].to_string(), "/ip4/127.0.0.1/tcp/1");
        assert_eq!(parsed[1].to_string(), "/ip4/10.0.0.1/tcp/2");
    }

    #[test]
    fn parse_list_enforces_hard_cap() {
        let raw: String = (0..64)
            .map(|i| format!("/ip4/127.0.0.1/tcp/{}", 1000 + i))
            .collect::<Vec<_>>()
            .join(",");
        let parsed = parse_multiaddr_list(&raw);
        assert_eq!(parsed.len(), MAX_BOOTSTRAP_PEERS, "cap enforced");
    }

    #[test]
    fn embedded_default_list_is_valid_or_empty() {
        // Every embedded entry must parse; until bootstrap VMs are published
        // the list is intentionally empty.
        for entry in EMBEDDED_BOOTSTRAP_MULTIADDRS {
            assert!(
                entry.parse::<Multiaddr>().is_ok(),
                "embedded bootstrap entry must be a valid multiaddr: {entry}"
            );
        }
    }

    #[test]
    fn resolve_uses_env_override_without_network() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mbhub_boot_env_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cache = dir.join("bootstrap-cache.json");
        unsafe {
            std::env::set_var("MBHUB_BOOTSTRAP_CACHE", &cache);
            std::env::set_var("MBHUB_BOOTSTRAP_PEERS", "/ip4/127.0.0.1/tcp/37777");
        }
        let list = resolve();
        assert_eq!(list.source, BootstrapSource::Env);
        assert_eq!(list.addresses.len(), 1);
        assert_eq!(list.addresses[0].to_string(), "/ip4/127.0.0.1/tcp/37777");
        unsafe {
            std::env::remove_var("MBHUB_BOOTSTRAP_PEERS");
            std::env::remove_var("MBHUB_BOOTSTRAP_CACHE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_roundtrip_is_tolerant() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mbhub_boot_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("bootstrap-cache.json");
        std::fs::create_dir_all(&dir).unwrap();
        unsafe {
            std::env::set_var("MBHUB_BOOTSTRAP_CACHE", &cache);
        }

        // No cache file → empty read (no panic).
        assert!(read_cache().is_empty());

        // Corrupt cache → empty read (no panic).
        std::fs::write(&cache, b"{not json").unwrap();
        assert!(read_cache().is_empty());

        // Valid cache round-trips.
        write_cache(&["/ip4/1.2.3.4/tcp/37777".parse::<Multiaddr>().unwrap()]);
        let read = read_cache();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].to_string(), "/ip4/1.2.3.4/tcp/37777");

        // Written cache is owner-only on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "bootstrap cache must be owner-only");
        }

        unsafe {
            std::env::remove_var("MBHUB_BOOTSTRAP_CACHE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
