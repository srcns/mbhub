//! SQLite store. Opens (and, when empty, seeds) a local database and
//! reads inference records out of it.
//!
//! ### Scalability & Lazy Loading Roadmap:
//! When the store scales to millions of records:
//! 1. **Virtual Windowing / Chunked Paging:** Instead of loading all rows into RAM,
//!    queries will use sliding window bounds:
//!    `SELECT timestamp, similarity, question, content FROM inferences ORDER BY similarity DESC LIMIT :limit OFFSET :offset`.
//! 2. **Bounded Sliding Window Cache:** In-memory `Vec<InferenceRecord>` maintains only
//!    the visible viewport + a small buffer (~100-200 items around the scroll cursor).
//!    Off-screen records are evicted from memory to keep RAM usage constant (\(O(1)\) memory).
//! 3. **Virtual List State:** Total record count (`SELECT COUNT(*)`) drives the scrollbar
//!    and position calculations while fetching slices lazily as the user scrolls.

use chrono::{Datelike, Local, TimeZone, Timelike};
use rusqlite::{params, Connection};

use crate::model::{InferenceRecord, Ts};
use crate::seed::SEED_ENTRIES;

/// Database path, overridable via `MBHUB_DB`.
/// In unit tests, uses an isolated temporary database to prevent data poisoning.
/// In production, uses `./mbhub.db` if present in cwd, or `~/.mbhub/mbhub.db`.
pub fn db_path() -> String {
    if let Ok(p) = std::env::var("MBHUB_DB") {
        return p;
    }
    #[cfg(test)]
    {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("mbhub_test_db_{}.sqlite", pid));
        return path.to_string_lossy().to_string();
    }
    #[cfg(not(test))]
    {
        if std::path::Path::new("mbhub.db").exists() {
            return "mbhub.db".to_string();
        }
        if let Some(home) = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()) {
            let dir = std::path::PathBuf::from(home).join(".mbhub");
            let _ = std::fs::create_dir_all(&dir);
            return dir.join("mbhub.db").to_string_lossy().to_string();
        }
        "mbhub.db".to_string()
    }
}

fn open() -> Connection {
    let conn = Connection::open(db_path()).expect("failed to open the MBHub sqlite database");
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    conn
}

fn init(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            val TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS inferences (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp  INTEGER NOT NULL,   -- unix epoch seconds
            similarity REAL    NOT NULL,   -- 1.0 .. 99.999
            question   TEXT    NOT NULL,   -- concise query (<= 80 chars)
            content    TEXT    NOT NULL,   -- full markdown response
            simhash    INTEGER NOT NULL DEFAULT 0,
            provider   TEXT    NOT NULL DEFAULT 'OpenAI',
            model      TEXT    NOT NULL DEFAULT 'gpt-4o',
            is_truncated INTEGER NOT NULL DEFAULT 0
         );
         -- The user's own past questions (SimHash only, never the raw text
         -- beyond the record itself). Drives Query Locality ordering.
         CREATE TABLE IF NOT EXISTS profile (
            simhash   INTEGER PRIMARY KEY,
            last_used INTEGER NOT NULL
         );
         -- Cryptographic Unidirectional Tombstones (Negative Signals).
         -- Records marked here are permanently excluded from L1 cache hits,
         -- dropped upon inbound swarm receipt, and never served.
         CREATE TABLE IF NOT EXISTS tombstones (
            content_hash TEXT PRIMARY KEY,
            simhash      INTEGER NOT NULL,
            timestamp    INTEGER NOT NULL,
            reason       TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_tombstones_hash ON tombstones(content_hash);
         CREATE INDEX IF NOT EXISTS idx_tombstones_simhash ON tombstones(simhash);",
    )
    .expect("failed to initialize the inferences schema and indexes");

    // Migration helper: add question column if upgrading from an older DB
    let has_question: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'question'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_question {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN question TEXT NOT NULL DEFAULT ''",
            [],
        );
    }

    // Migration helper: add simhash column if upgrading from an older DB
    let has_simhash: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'simhash'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_simhash {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN simhash INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }

    // Migration helper: add provider column if upgrading from an older DB
    let has_provider: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'provider'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_provider {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN provider TEXT NOT NULL DEFAULT 'OpenAI'",
            [],
        );
    }

    // Migration helper: add model column if upgrading from an older DB
    let has_model: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'model'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_model {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN model TEXT NOT NULL DEFAULT 'gpt-4o'",
            [],
        );
    }

    // Migration: add content_hash column for BLAKE3 content-addressing.
    // Provides tamper detection and content-based identity for every record.
    let has_content_hash: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'content_hash'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_content_hash {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''",
            [],
        );
    }

    // Migration: add is_swarm column marking swarm-sourced (unverified) records.
    let has_is_swarm: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'is_swarm'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_is_swarm {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN is_swarm INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }

    // Migration: add locality column — similarity of each record's question to
    // the user's own past questions (Query Locality score).
    let has_locality: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'locality'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_locality {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN locality REAL NOT NULL DEFAULT 0",
            [],
        );
    }

    // Migration: add is_truncated column marking cut-off or interrupted inferences
    let has_is_truncated: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'is_truncated'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_is_truncated {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN is_truncated INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }

    // Migration: add publish_candidate column (Phase 0 — Privacy Gate).
    // Only locally generated questions (is_swarm = 0) may be marked as candidates.
    let has_publish_candidate: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'publish_candidate'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_publish_candidate {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN publish_candidate INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // Mark existing local inferences as candidates
        let _ = conn.execute(
            "UPDATE inferences SET publish_candidate = 1 WHERE is_swarm = 0",
            [],
        );
    }

    // Migration: add published_at timestamp column
    let has_published_at: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inferences') WHERE name = 'published_at'",
            [],
            |r| {
                let count: i64 = r.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if !has_published_at {
        let _ = conn.execute(
            "ALTER TABLE inferences ADD COLUMN published_at INTEGER DEFAULT NULL",
            [],
        );
    }

    // Default pointer for blog exports
    let _ = conn.execute(
        "INSERT OR IGNORE INTO meta (key, val) VALUES ('last_blog_export_id', '0')",
        [],
    );

    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_inferences_ts_sim ON inferences (timestamp DESC, similarity DESC)",
        [],
    );

    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_inferences_simhash ON inferences (simhash)",
        [],
    );

    // Query Locality ordering index (Memory list + locality-aware eviction).
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_inferences_locality ON inferences (locality DESC, timestamp DESC)",
        [],
    );

    // Dedupe / integrity index: content-addressing makes replays idempotent.
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_inferences_content_hash ON inferences (content_hash)",
        [],
    );

    // Backfill migration: legacy records predating Phase 2 have an empty
    // content_hash and cannot be served to peers (receivers reject them).
    // Compute the canonical hash from the actual stored fields once, so
    // pre-existing knowledge becomes integrity-verifiable and shareable.
    backfill_content_hashes(conn);

    // Backfill migration: rows restored by the web-archive rehydrator (or any
    // legacy writer) may carry simhash = 0. Without a real fingerprint they
    // can never be served to the swarm (query responses are verified against
    // the asker's SimHash). Recompute it deterministically from the question.
    backfill_simhashes(conn);
}

/// One-time backfill of content hashes for legacy rows (empty content_hash).
/// Hashes are computed over the same sanitized canonical fields as the write
/// path, so backfilled rows verify identically to freshly saved ones.
fn backfill_content_hashes(conn: &Connection) {
    let missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inferences WHERE content_hash = '' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if missing == 0 {
        return;
    }

    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(_) => return,
    };
    {
        let mut select = match tx.prepare(
            "SELECT id, question, content, provider, model FROM inferences WHERE content_hash = ''",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut update = match tx.prepare("UPDATE inferences SET content_hash = ?1 WHERE id = ?2") {
            Ok(s) => s,
            Err(_) => return,
        };

        let rows: Vec<(i64, String)> = select
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let question: String = row.get(1)?;
                let content: String = row.get(2)?;
                let provider: String = row.get(3)?;
                let model: String = row.get(4)?;
                let hash = crate::content_hash::compute_content_hash(
                    &crate::sanitize::strip_control_chars(&question),
                    &crate::sanitize::strip_control_chars(&content),
                    &crate::sanitize::strip_control_chars(&provider),
                    &crate::sanitize::strip_control_chars(&model),
                );
                Ok((id, hash))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (id, hash) in rows {
            let _ = update.execute(params![hash, id]);
        }
    }
    let _ = tx.commit();
}

/// One-time backfill of SimHash fingerprints for rows stored with simhash = 0
/// (e.g. records restored from the web archive before the fingerprint was
/// preserved). The fingerprint is computed from the question text, exactly as
/// the live write path does.
fn backfill_simhashes(conn: &Connection) {
    let missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inferences WHERE simhash = 0 LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if missing == 0 {
        return;
    }

    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(_) => return,
    };
    {
        let mut select = match tx.prepare("SELECT id, question FROM inferences WHERE simhash = 0") {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut update = match tx.prepare("UPDATE inferences SET simhash = ?1 WHERE id = ?2") {
            Ok(s) => s,
            Err(_) => return,
        };

        let rows: Vec<(i64, u64)> = select
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let question: String = row.get(1)?;
                let hash = crate::simhash::compute_simhash(&question);
                Ok((id, hash))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (id, hash) in rows {
            let _ = update.execute(params![hash as i64, id]);
        }
    }
    let _ = tx.commit();
}

/// Returns the total count of inference records stored in the SQLite database (excluding tombstones).
pub fn count_records() -> usize {
    let conn = open();
    init(&conn);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inferences
             WHERE NOT EXISTS (
                 SELECT 1 FROM tombstones
                 WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
             )",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    count as usize
}

/// Memory list ordering modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOrder {
    /// Query Locality: records most similar to the user's past questions on
    /// top (locality DESC, then newest first).
    Locality,
    /// Blind Swarm: no relevance tracking — newest first.
    Recent,
}

/// Loads a bounded slice of inference records using sliding window bounds.
///
/// The database stores EVERY verified answer regardless of relevance; ordering
/// is what differs:
/// - `Locality` (Query Locality mode): most similar to the user's past
///   questions first, so the shard reads like a personalized memory.
/// - `Recent` (Blind Swarm mode): newest first, no relevance signal.
pub fn load_records_window_ordered(
    offset: usize,
    limit: usize,
    order: ListOrder,
) -> Vec<InferenceRecord> {
    let conn = open();
    init(&conn);

    let order_by = match order {
        ListOrder::Locality => "locality DESC, timestamp DESC",
        ListOrder::Recent => "timestamp DESC",
    };

    let sql = format!(
        "SELECT timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate
         FROM inferences
         WHERE NOT EXISTS (
             SELECT 1 FROM tombstones
             WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
         )
         ORDER BY {order_by}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn
        .prepare(&sql)
        .expect("failed to prepare bounded inference query");

    let rows = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            let secs: i64 = row.get(0)?;
            let sim: f64 = row.get(1)?;
            let question: String = row.get(2)?;
            let content: String = row.get(3)?;
            let simhash: i64 = row.get(4).unwrap_or(0);
            let provider: String = row.get(5).unwrap_or_else(|_| "OpenAI".to_string());
            let model: String = row.get(6).unwrap_or_else(|_| "gpt-4o".to_string());
            let content_hash: String = row.get(7).unwrap_or_default();
            let is_swarm: i64 = row.get(8).unwrap_or(0);
            let locality: f64 = row.get(9).unwrap_or(0.0);
            let is_truncated: i64 = row.get(10).unwrap_or(0);
            let publish_candidate: i64 = row.get(11).unwrap_or(0);
            Ok(InferenceRecord {
                ts: from_unix(secs),
                similarity: sim as f32,
                question,
                content,
                simhash: simhash as u64,
                provider,
                model,
                content_hash,
                is_swarm: is_swarm != 0,
                locality: locality as f32,
                is_truncated: is_truncated != 0,
                publish_candidate: publish_candidate != 0,
            })
        })
        .expect("failed to query bounded inferences");

    rows.filter_map(|r| r.ok()).collect()
}

/// Legacy entry point: Query Locality ordering.
pub fn load_records_window(offset: usize, limit: usize) -> Vec<InferenceRecord> {
    load_records_window_ordered(offset, limit, ListOrder::Locality)
}

/// Blind Swarm ordering (newest first, no relevance tracking).
pub fn load_records_window_recent(offset: usize, limit: usize) -> Vec<InferenceRecord> {
    load_records_window_ordered(offset, limit, ListOrder::Recent)
}

/// Fetches a single inference record by its sorted rank index.
pub fn get_record_at(index: usize) -> Option<InferenceRecord> {
    load_records_window(index, 1).into_iter().next()
}

/// Fetches a single inference record by rank under Blind Swarm ordering.
pub fn get_record_at_recent(index: usize) -> Option<InferenceRecord> {
    load_records_window_recent(index, 1).into_iter().next()
}

/// Load all records, sorted by Query Locality descending.
#[allow(dead_code)]
pub fn load_records() -> Vec<InferenceRecord> {
    load_records_window(0, usize::MAX)
}

/// Saves a newly completed inference response into the SQLite database
/// using the precomputed SimHash summary, along with the generator model and provider.
///
/// Security layers applied before write:
/// 1. Terminal escape sanitization (defense-in-depth for future UI surfaces).
/// 2. BLAKE3 content-addressing for tamper detection.
///
/// Locally produced inferences always insert (no dedupe): re-asking a question
/// refreshes the record timestamp, which is the intended L1 semantics.
#[allow(dead_code)]
pub fn save_inference(
    question: &str,
    content: &str,
    simhash: u64,
    provider: &str,
    model: &str,
) -> Option<InferenceRecord> {
    save_inference_internal(question, content, simhash, provider, model, None, false, false)
}

pub fn save_inference_with_truncated(
    question: &str,
    content: &str,
    simhash: u64,
    provider: &str,
    model: &str,
    is_truncated: bool,
) -> Option<InferenceRecord> {
    save_inference_internal(question, content, simhash, provider, model, None, false, is_truncated)
}

/// Checks whether a content hash is marked with a tombstone (negative signal).
pub fn is_tombstoned(content_hash: &str) -> bool {
    if content_hash.is_empty() {
        return false;
    }
    let conn = open();
    init(&conn);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tombstones WHERE content_hash = ?1",
            params![content_hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    count > 0
}

/// Records a tombstone for a content hash and simhash, permanently preventing
/// it from being cached or served, and deletes any matching inference from SQLite.
pub fn add_tombstone(content_hash: &str, simhash: u64, reason: &str) {
    if content_hash.is_empty() {
        return;
    }
    let conn = open();
    init(&conn);
    let now = chrono::Local::now().timestamp();
    let _ = conn.execute(
        "INSERT OR IGNORE INTO tombstones (content_hash, simhash, timestamp, reason) VALUES (?1, ?2, ?3, ?4)",
        params![content_hash, simhash as i64, now, reason],
    );
    let _ = conn.execute(
        "DELETE FROM inferences WHERE content_hash = ?1",
        params![content_hash],
    );
}

/// Deletes a record from local storage and adds a tombstone.
/// Returns the content hash and simhash of the deleted record.
pub fn delete_and_tombstone_record(record: &InferenceRecord, reason: &str) -> (String, u64) {
    let mut hash = record.content_hash.clone();
    if hash.is_empty() {
        hash = crate::content_hash::compute_content_hash(
            &record.question,
            &record.content,
            &record.provider,
            &record.model,
        );
    }
    add_tombstone(&hash, record.simhash, reason);
    let conn = open();
    init(&conn);
    let _ = conn.execute(
        "DELETE FROM inferences WHERE content_hash = ?1 OR (question = ?2 AND simhash = ?3)",
        params![hash, record.question, record.simhash as i64],
    );
    (hash, record.simhash)
}

/// Saves an inference received from the P2P swarm, after verifying that the
/// sender-claimed `claimed_content_hash` matches the actual sanitized fields.
///
/// Returns `None` (and writes nothing) when:
/// - the content hash does not verify (tampered or spoofed payload), or
/// - the exact same content hash is already stored (replay dedupe).
///
/// Receiver-side gate ordering is the caller's responsibility (DLP + content
/// safety screening happen in `app.rs` before this call).
pub fn save_swarm_inference(
    question: &str,
    content: &str,
    simhash: u64,
    provider: &str,
    model: &str,
    claimed_content_hash: &str,
) -> Option<InferenceRecord> {
    save_inference_internal(
        question,
        content,
        simhash,
        provider,
        model,
        Some(claimed_content_hash),
        true,
        false,
    )
}

fn save_inference_internal(
    question: &str,
    content: &str,
    simhash: u64,
    provider: &str,
    model: &str,
    claimed_content_hash: Option<&str>,
    is_swarm: bool,
    is_truncated: bool,
) -> Option<InferenceRecord> {
    // Defense-in-depth: sanitize before persisting so future UI surfaces
    // (GUI, web export) never encounter raw escape sequences from the DB.
    let clean_question = crate::sanitize::strip_control_chars(question);
    let clean_content = crate::sanitize::strip_control_chars(content);
    let clean_provider = crate::sanitize::strip_control_chars(provider);
    let clean_model = crate::sanitize::strip_control_chars(model);

    // Anti-Poison Hard Gate: reject empty or uninformative content / question
    if clean_content.trim().is_empty()
        || clean_content.trim().len() < 10
        || clean_question.trim().is_empty()
        || clean_question.trim().len() < 3
    {
        return None;
    }

    let content_hash = crate::content_hash::compute_content_hash(
        &clean_question,
        &clean_content,
        &clean_provider,
        &clean_model,
    );

    // Tombstone Check: reject any content hash that has been tombstoned
    if is_tombstoned(&content_hash) {
        return None;
    }

    let conn = open();
    init(&conn);

    // Swarm path only: content integrity verification and
    // replay dedupe. Local saves bypass both — a user re-asking
    // a question legitimately refreshes the local record.
    if claimed_content_hash.is_some() {
        if let Some(claimed) = claimed_content_hash {
            if claimed.is_empty() || claimed != content_hash {
                return None;
            }
        }

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM inferences WHERE content_hash = ?1 AND content_hash != ''",
                params![content_hash],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists > 0 {
            return None;
        }
    }

    let ts = Local::now().timestamp();
    let sim = 100.0f32;

    // Query Locality: score the incoming record against the user's own past
    // questions so Memory ordering works immediately (full recompute happens
    // on the next ASK submission).
    let locality = locality_score(simhash, &load_profile_hashes());

    let publish_candidate = if !is_swarm { 1 } else { 0 };

    conn.execute(
        "INSERT INTO inferences (timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            ts,
            sim as f64,
            &clean_question,
            &clean_content,
            simhash as i64,
            &clean_provider,
            &clean_model,
            &content_hash,
            is_swarm as i64,
            locality as f64,
            is_truncated as i64,
            publish_candidate,
        ],
    )
    .expect("failed to insert new inference");

    Some(InferenceRecord {
        ts: from_unix(ts),
        similarity: sim,
        question: clean_question,
        content: clean_content,
        simhash,
        provider: clean_provider,
        model: clean_model,
        content_hash,
        is_swarm,
        locality,
        is_truncated,
        publish_candidate: publish_candidate != 0,
    })
}

/// Returns true when a record with this content hash is already stored.
/// Used by the app layer for replay detection before enqueueing swarm writes.
pub fn inference_exists(content_hash: &str) -> bool {
    if content_hash.is_empty() {
        return false;
    }
    let conn = open();
    init(&conn);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inferences WHERE content_hash = ?1",
            params![content_hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    count > 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Query Locality: user's own question profile drives Memory ordering and
// locality-aware eviction. The DB stores EVERY verified answer regardless of
// relevance (the Hit Rate Threshold only gates direct on-screen display);
// relevance is expressed as the `locality` score per record.
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of distinct user questions kept in the profile.
/// Bounded so recomputation stays O(records × PROFILE_LIMIT), O(1) RAM.
const PROFILE_LIMIT: usize = 100;

/// Records one of the user's own asked questions (by SimHash) in the profile
/// and refreshes the locality score of every stored record. Called on every
/// ASK submission — the SQLite shard lives and re-orders continuously with
/// the user's query history.
pub fn record_profile_query(simhash: u64) {
    if simhash == 0 {
        return;
    }
    let conn = open();
    init(&conn);

    let now = chrono::Local::now().timestamp();
    let _ = conn.execute(
        "INSERT INTO profile (simhash, last_used) VALUES (?1, ?2)
         ON CONFLICT(simhash) DO UPDATE SET last_used = excluded.last_used",
        params![simhash as i64, now],
    );

    // Bound the profile: keep the 100 most recently used questions.
    let _ = conn.execute(
        "DELETE FROM profile WHERE simhash NOT IN (
             SELECT simhash FROM profile ORDER BY last_used DESC LIMIT ?1
         )",
        params![PROFILE_LIMIT as i64],
    );

    drop(conn);
    recompute_locality();
}

/// Empties the user's query profile (used when switching sharding modes:
/// Blind Swarm promises zero query tracking).
pub fn clear_profile() {
    let conn = open();
    init(&conn);
    let _ = conn.execute("DELETE FROM profile", []);
}

/// Loads the current profile SimHashes (most recent first).
pub fn load_profile_hashes() -> Vec<u64> {
    let conn = open();
    init(&conn);
    let mut stmt = match conn.prepare("SELECT simhash FROM profile ORDER BY last_used DESC") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([], |r| r.get::<_, i64>(0)) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).map(|v| v as u64).collect()
}

/// Similarity of one record's question SimHash against the user's profile:
/// the best (max) Hamming similarity across past questions, 0..100.
pub fn locality_score(simhash: u64, profile: &[u64]) -> f32 {
    if simhash == 0 || profile.is_empty() {
        return 0.0;
    }
    let mut best: f32 = 0.0;
    for &p in profile {
        let sim = crate::simhash::similarity(simhash, p);
        if sim > best {
            best = sim;
        }
    }
    best
}

/// Streaming recomputation of every record's locality score from the current
/// profile. Runs inside a single transaction with O(1) RAM. At the project's
/// current scale this is milliseconds; for very large shards it is the natural
/// candidate for background/chunked processing (Phase 3 note).
pub fn recompute_locality() {
    let conn = open();
    init(&conn);
    let profile = load_profile_hashes();
    if profile.is_empty() {
        return;
    }

    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(_) => return,
    };
    {
        let mut select = match tx.prepare("SELECT id, simhash FROM inferences") {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut update = match tx.prepare("UPDATE inferences SET locality = ?1 WHERE id = ?2") {
            Ok(s) => s,
            Err(_) => return,
        };

        let ids: Vec<(i64, f32)> = select
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let simhash: i64 = row.get(1).unwrap_or(0);
                Ok((id, locality_score(simhash as u64, &profile)))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (id, score) in ids {
            let _ = update.execute(params![score as f64, id]);
        }
    }
    let _ = tx.commit();
}

/// Finds the best matching candidate in local storage whose SimHash similarity meets
/// or exceeds `min_similarity` percentage, using a precomputed 64-bit query SimHash.
/// Uses streaming cursor evaluation to maintain constant O(1) memory overhead.
pub fn find_best_match_by_hash(query_hash: u64, min_similarity: f32) -> Option<InferenceRecord> {
    let conn = open();
    init(&conn);

    let mut stmt = conn
        .prepare(
            "SELECT timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate
             FROM inferences
             WHERE is_truncated = 0 AND NOT EXISTS (
                 SELECT 1 FROM tombstones
                 WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
             )
             ORDER BY similarity DESC",
        )
        .ok()?;

    let rows = stmt
        .query_map([], |row| {
            let secs: i64 = row.get(0)?;
            let sim: f64 = row.get(1)?;
            let question: String = row.get(2)?;
            let content: String = row.get(3)?;
            let simhash: i64 = row.get(4).unwrap_or(0);
            let provider: String = row.get(5).unwrap_or_else(|_| "OpenAI".to_string());
            let model: String = row.get(6).unwrap_or_else(|_| "gpt-4o".to_string());
            let content_hash: String = row.get(7).unwrap_or_default();
            let is_swarm: i64 = row.get(8).unwrap_or(0);
            let locality: f64 = row.get(9).unwrap_or(0.0);
            let is_truncated: i64 = row.get(10).unwrap_or(0);
            let publish_candidate: i64 = row.get(11).unwrap_or(0);
            Ok(InferenceRecord {
                ts: from_unix(secs),
                similarity: sim as f32,
                question,
                content,
                simhash: simhash as u64,
                provider,
                model,
                content_hash,
                is_swarm: is_swarm != 0,
                locality: locality as f32,
                is_truncated: is_truncated != 0,
                publish_candidate: publish_candidate != 0,
            })
        })
        .ok()?;

    let mut best: Option<(f32, InferenceRecord)> = None;

    for row_res in rows {
        let mut r = match row_res {
            Ok(rec) => rec,
            Err(_) => continue,
        };

        let stored_hash = if r.simhash != 0 {
            r.simhash
        } else {
            crate::simhash::compute_simhash(&r.question)
        };

        let sim = crate::simhash::similarity(query_hash, stored_hash);
        if sim >= min_similarity {
            if best.as_ref().map_or(true, |(b_sim, _)| sim > *b_sim) {
                r.similarity = sim;
                best = Some((sim, r));
            }
        }
    }

    best.map(|(_, r)| r)
}

/// Finds the best matching candidate by query string.
#[allow(dead_code)]
pub fn find_best_match(query: &str, min_similarity: f32) -> Option<InferenceRecord> {
    find_best_match_query_fresh(query, min_similarity, None)
}

/// Freshness-aware variant of `find_best_match_by_hash`. Records older than `min_timestamp` are skipped;
/// `None` disables the freshness filter (the `Any time` setting).
#[allow(dead_code)]
pub fn find_best_match_by_hash_fresh(
    query_hash: u64,
    min_similarity: f32,
    min_timestamp: Option<i64>,
) -> Option<InferenceRecord> {
    if min_timestamp.is_none() {
        return find_best_match_by_hash(query_hash, min_similarity);
    }

    let conn = open();
    init(&conn);

    let mut stmt = conn
        .prepare(
            "SELECT timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate
             FROM inferences
             WHERE timestamp >= ?1
               AND is_truncated = 0
               AND NOT EXISTS (
                   SELECT 1 FROM tombstones
                   WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
               )
             ORDER BY similarity DESC",
        )
        .ok()?;

    let rows = stmt
        .query_map(params![min_timestamp.unwrap_or(0)], |row| {
            let secs: i64 = row.get(0)?;
            let sim: f64 = row.get(1)?;
            let question: String = row.get(2)?;
            let content: String = row.get(3)?;
            let simhash: i64 = row.get(4).unwrap_or(0);
            let provider: String = row.get(5).unwrap_or_else(|_| "OpenAI".to_string());
            let model: String = row.get(6).unwrap_or_else(|_| "gpt-4o".to_string());
            let content_hash: String = row.get(7).unwrap_or_default();
            let is_swarm: i64 = row.get(8).unwrap_or(0);
            let locality: f64 = row.get(9).unwrap_or(0.0);
            let is_truncated: i64 = row.get(10).unwrap_or(0);
            let publish_candidate: i64 = row.get(11).unwrap_or(0);
            Ok(InferenceRecord {
                ts: from_unix(secs),
                similarity: sim as f32,
                question,
                content,
                simhash: simhash as u64,
                provider,
                model,
                content_hash,
                is_swarm: is_swarm != 0,
                locality: locality as f32,
                is_truncated: is_truncated != 0,
                publish_candidate: publish_candidate != 0,
            })
        })
        .ok()?;

    let mut best: Option<(f32, InferenceRecord)> = None;

    for row_res in rows {
        let mut r = match row_res {
            Ok(rec) => rec,
            Err(_) => continue,
        };

        let stored_hash = if r.simhash != 0 {
            r.simhash
        } else {
            crate::simhash::compute_simhash(&r.question)
        };

        let sim = crate::simhash::similarity(query_hash, stored_hash);
        if sim >= min_similarity {
            if best.as_ref().map_or(true, |(b_sim, _)| sim > *b_sim) {
                r.similarity = sim;
                best = Some((sim, r));
            }
        }
    }

    best.map(|(_, r)| r)
}

/// Query-string-aware match lookup combining SimHash similarity, short-query guard,
/// and freshness filtering while excluding tombstones.
pub fn find_best_match_query_fresh(
    query: &str,
    min_similarity: f32,
    min_timestamp: Option<i64>,
) -> Option<InferenceRecord> {
    let query_hash = crate::simhash::compute_simhash(query);
    let conn = open();
    init(&conn);

    let sql = if let Some(min_ts) = min_timestamp {
        format!(
            "SELECT timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate
             FROM inferences
             WHERE timestamp >= {min_ts}
               AND is_truncated = 0
               AND NOT EXISTS (
                   SELECT 1 FROM tombstones
                   WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
               )
             ORDER BY similarity DESC"
        )
    } else {
        "SELECT timestamp, similarity, question, content, simhash, provider, model, content_hash, is_swarm, locality, is_truncated, publish_candidate
         FROM inferences
         WHERE is_truncated = 0
           AND NOT EXISTS (
             SELECT 1 FROM tombstones
             WHERE tombstones.content_hash = inferences.content_hash AND inferences.content_hash != ''
         )
         ORDER BY similarity DESC".to_string()
    };

    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map([], |row| {
            let secs: i64 = row.get(0)?;
            let sim: f64 = row.get(1)?;
            let question: String = row.get(2)?;
            let content: String = row.get(3)?;
            let simhash: i64 = row.get(4).unwrap_or(0);
            let provider: String = row.get(5).unwrap_or_else(|_| "OpenAI".to_string());
            let model: String = row.get(6).unwrap_or_else(|_| "gpt-4o".to_string());
            let content_hash: String = row.get(7).unwrap_or_default();
            let is_swarm: i64 = row.get(8).unwrap_or(0);
            let locality: f64 = row.get(9).unwrap_or(0.0);
            let is_truncated: i64 = row.get(10).unwrap_or(0);
            let publish_candidate: i64 = row.get(11).unwrap_or(0);
            Ok(InferenceRecord {
                ts: from_unix(secs),
                similarity: sim as f32,
                question,
                content,
                simhash: simhash as u64,
                provider,
                model,
                content_hash,
                is_swarm: is_swarm != 0,
                locality: locality as f32,
                is_truncated: is_truncated != 0,
                publish_candidate: publish_candidate != 0,
            })
        })
        .ok()?;

    let mut best: Option<(f32, InferenceRecord)> = None;

    for row_res in rows {
        let mut r = match row_res {
            Ok(rec) => rec,
            Err(_) => continue,
        };

        if crate::simhash::matches_query(query, &r.question, min_similarity) {
            let stored_hash = if r.simhash != 0 {
                r.simhash
            } else {
                crate::simhash::compute_simhash(&r.question)
            };
            let sim = crate::simhash::similarity(query_hash, stored_hash);
            if best.as_ref().map_or(true, |(b_sim, _)| sim > *b_sim) {
                r.similarity = sim;
                best = Some((sim, r));
            }
        }
    }

    best.map(|(_, r)| r)
}

/// Read a key-value pair from the persistent meta table.
pub fn get_meta(key: &str) -> Option<String> {
    let conn = open();
    init(&conn);
    conn.query_row(
        "SELECT val FROM meta WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Write a key-value pair to the persistent meta table.
pub fn set_meta(key: &str, val: &str) {
    let conn = open();
    init(&conn);
    let _ = conn.execute(
        "INSERT OR REPLACE INTO meta (key, val) VALUES (?1, ?2)",
        params![key, val],
    );
}

/// Get stored API key for a specific provider.
pub fn get_provider_api_key(provider_name: &str) -> String {
    get_meta(&format!("apikey:{}", provider_name)).unwrap_or_default()
}

/// Set stored API key for a specific provider.
pub fn set_provider_api_key(provider_name: &str, key: &str) {
    set_meta(&format!("apikey:{}", provider_name), key);
}

/// Get stored model for a specific provider.
pub fn get_provider_model(provider_name: &str) -> Option<String> {
    get_meta(&format!("model:{}", provider_name))
}

/// Set stored model for a specific provider.
pub fn set_provider_model(provider_name: &str, model: &str) {
    set_meta(&format!("model:{}", provider_name), model);
}

/// Loads all per-provider API keys into a HashMap.
pub fn load_provider_keys() -> std::collections::HashMap<String, String> {
    let conn = open();
    init(&conn);
    let mut map = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT key, val FROM meta WHERE key LIKE 'apikey:%'") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
            for (k, v) in rows.flatten() {
                if let Some(prov) = k.strip_prefix("apikey:") {
                    map.insert(prov.to_string(), v);
                }
            }
        }
    }
    map
}

/// Loads all per-provider selected models into a HashMap.
pub fn load_provider_models() -> std::collections::HashMap<String, String> {
    let conn = open();
    init(&conn);
    let mut map = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT key, val FROM meta WHERE key LIKE 'model:%'") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
            for (k, v) in rows.flatten() {
                if let Some(prov) = k.strip_prefix("model:") {
                    map.insert(prov.to_string(), v);
                }
            }
        }
    }
    map
}

/// Purge all records from local storage. Keeps the store clean and empty until
/// new queries arrive.
pub fn clear_all() -> usize {
    let conn = open();
    init(&conn);
    let _ = conn.execute(
        "INSERT OR REPLACE INTO meta (key, val) VALUES ('initialized', '1')",
        [],
    );
    conn.execute("DELETE FROM inferences", []).unwrap_or(0)
}

/// Clear all metadata and settings from meta table (used for test isolation).
#[allow(dead_code)]
pub fn clear_meta() {
    let conn = open();
    init(&conn);
    let _ = conn.execute("DELETE FROM meta", []);
}

/// Clear all tombstones from tombstones table (used for test isolation).
#[allow(dead_code)]
pub fn clear_tombstones() {
    let conn = open();
    init(&conn);
    let _ = conn.execute("DELETE FROM tombstones", []);
}

/// Force (re)generate the seed corpus. Returns the number of rows inserted.
pub fn reseed() -> usize {
    let conn = open();
    init(&conn);
    seed(&conn)
}

/// Exports a complete snapshot of the current database to `dest_path`.
pub fn backup_to_file(dest_path: &std::path::Path) -> Result<(), rusqlite::Error> {
    let src = open();
    init(&src);
    let mut dst = Connection::open(dest_path)?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        backup.run_to_completion(100, std::time::Duration::from_millis(10), None)?;
    }
    Ok(())
}

/// Restores the current database from an existing backup at `src_path`.
/// Returns the number of inference records restored.
pub fn restore_from_file(src_path: &std::path::Path) -> Result<usize, rusqlite::Error> {
    let src = Connection::open(src_path)?;
    let mut dst = open();
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        backup.run_to_completion(100, std::time::Duration::from_millis(10), None)?;
    }

    // Ensure schema migrations & indices are complete on the restored database
    init(&dst);
    let count: i64 = dst
        .query_row("SELECT COUNT(*) FROM inferences", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(count as usize)
}

/// Enforces the user-configured fixed-GB local storage ceiling.
///
/// Eviction follows the sharding mode:
/// - `locality_first = true` (Query Locality): the least relevant records
///   (lowest similarity to the user's past questions) are evicted first, so the
///   most compatible Q&As stay alive at the top of the shard.
/// - `locality_first = false` (Blind Swarm): oldest-first eviction; no
///   relevance signal is tracked.
///
/// Called after every write path and at startup. Returns the number of pruned rows.
pub fn enforce_storage_limit_gb(max_gb: u64, locality_first: bool) -> u64 {
    let cap_bytes = (max_gb as u64).saturating_mul(1_000_000_000).max(10_000_000);
    enforce_storage_limit_bytes(cap_bytes, locality_first)
}

/// Internal byte-accurate variant (used directly by tests with tiny caps).
///
/// Eviction is estimate-based and convergent. The physical SQLite file does not
/// shrink during DELETEs (freed pages sit on the freelist), so convergence is
/// measured against the *effective* size (physical size minus freelist pages).
/// Chunks are deleted in eviction order (locality-first for Query Locality,
/// oldest-first for Blind Swarm); a final bounded VACUUM reclaims the space.
/// This preserves the most relevant records instead of nuking the whole shard.
pub fn enforce_storage_limit_bytes(max_bytes: u64, locality_first: bool) -> u64 {
    let conn = open();
    init(&conn);

    let db_path_str = db_path();
    let path = std::path::Path::new(&db_path_str);

    // Stored-data footprint only: main file + WAL. The -shm file is the WAL
    // shared-memory index (runtime state, ~32 KB, removed when connections
    // close) and must NOT count against the user's storage budget — doing so
    // would undercount the effective cap and evict everything.
    let physical_size = || {
        let mut s = file_size(path);
        s += file_size(&std::path::PathBuf::from(format!("{}-wal", db_path_str)));
        s
    };

    if physical_size() <= max_bytes {
        return 0;
    }

    let eviction_order = if locality_first {
        "locality ASC, timestamp ASC"
    } else {
        "timestamp ASC"
    };

    let mut pruned: u64 = 0;

    // Convergent loop: each iteration estimates how many rows to delete from
    // the CURRENT physical size, deletes them in eviction order, and VACUUMs
    // so the next iteration measures the true shrunken size. At most 3
    // VACUUMs per run — bounded, and only ever triggered by real overflow.
    for _outer in 0..3 {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let size = physical_size();
        if size <= max_bytes {
            break;
        }
        let rows: u64 = conn
            .query_row("SELECT COUNT(*) FROM inferences", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64;
        if rows == 0 {
            break;
        }

        // Estimate average bytes per row and derive how many rows to keep so
        // the file lands at ~90% of the cap; add slack for size variance.
        let avg = (size / rows).max(1);
        let keep_target = ((max_bytes as f64 * 0.9) / avg as f64).floor() as u64;
        let to_delete = rows.saturating_sub(keep_target).saturating_add(10).min(rows);

        if to_delete == 0 {
            break;
        }

        let mut deleted_this_round: u64 = 0;
        while deleted_this_round < to_delete {
            let chunk = (to_delete - deleted_this_round).min(500);
            let sql = format!(
                "DELETE FROM inferences WHERE id IN (
                     SELECT id FROM inferences ORDER BY {eviction_order} LIMIT {chunk}
                 )"
            );
            let removed = conn.execute(&sql, []).unwrap_or(0);
            if removed == 0 {
                break;
            }
            deleted_this_round += removed as u64;
            pruned += removed as u64;
        }

        // Shrink sequence (validated order for WAL databases):
        //   1. checkpoint merges the delete frames into the main file,
        //   2. VACUUM rebuilds it compactly (its pages land in the WAL),
        //   3. a final TRUNCATE checkpoint merges them back and shrinks.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let _ = conn.execute("VACUUM", []);
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }

    pruned
}

fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn seed(conn: &Connection) -> usize {
    let tx = conn.unchecked_transaction().expect("failed to begin seed tx");
    tx.execute("DELETE FROM inferences", [])
        .expect("failed to clear inferences");
    {
        let mut stmt = tx
            .prepare("INSERT INTO inferences (timestamp, similarity, question, content, simhash, provider, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .expect("failed to prepare insert");
        let base = base_epoch();
        for (i, entry) in SEED_ENTRIES.iter().enumerate() {
            // ~11 hours apart with deterministic jitter, walking back in time.
            let ts = base - (i as i64) * 39_600 - ((i * 37) % 5400) as i64;
            let sim = 1.0 + (pseudo(i as u32) % 98_999) as f64 / 1000.0;
            let simhash = crate::simhash::compute_simhash(&entry.question);
            stmt.execute(params![ts, sim, entry.question, entry.content, simhash as i64, "OpenAI", "gpt-4o"])
                .expect("failed to insert seed row");
        }
    }
    tx.commit().expect("failed to commit seed tx");
    SEED_ENTRIES.len()
}

/// Reference point for the seed timestamps, expressed as a local epoch.
fn base_epoch() -> i64 {
    Local
        .with_ymd_and_hms(2026, 1, 20, 20, 0, 0)
        .single()
        .expect("valid seed date")
        .timestamp()
}

/// Local-time components for a unix epoch, so the date-format selector can
/// reformat the value in the user's own timezone.
pub fn from_unix(secs: i64) -> Ts {
    let dt = Local.timestamp_opt(secs, 0).single().unwrap_or_else(|| {
        Local
            .with_ymd_and_hms(1970, 1, 1, 0, 0, 0)
            .single()
            .expect("epoch fallback")
    });
    Ts {
        year: dt.year() as u16,
        month: dt.month() as u8,
        day: dt.day() as u8,
        hour: dt.hour() as u8,
        minute: dt.minute() as u8,
    }
}

/// Small deterministic PRNG so the smoke data is reproducible.
fn pseudo(mut x: u32) -> u32 {
    x = x.wrapping_add(1).wrapping_mul(0x9E37_79B9);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    x
}
