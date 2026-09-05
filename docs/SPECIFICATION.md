# MBHub Master Technical Specification & Architecture Manual

**Version:** 1.0.0  
**Status:** Active Single Source of Truth  
**Date:** September 2026  
**Security Model:** Zero-Trust, Serverless, Cryptographically Verifiable, BYOK (Bring Your Own Key)

---

## 1. System Philosophy & Purpose

### 1.1 The Global Problem: Inference Waste and Centralization
Contemporary AI infrastructure relies on centralized server farms. Every second, millions of developers ask nearly identical technical questions:
* *"How does ownership work in Rust?"*
* *"How to implement distributed consensus using Raft?"*
* *"What are standard client-side cybersecurity measures in P2P networks?"*
* *"How does the SimHash algorithm work?"*

For each duplicate question, data centers re-execute identical tensor multiplication operations, burning megawatts of electricity, emitting carbon, and charging users per token. Furthermore, this collective intelligence is locked inside proprietary corporate walled gardens where user query habits are profiled.

### 1.2 The MBHub Vision: The Torrent of Thought
MBHub is a sovereign, serverless, peer-to-peer (P2P) distributed AI inference cache and collective memory network.

Just as BitTorrent transitioned file distribution from centralized servers to a decentralized mesh of peers, MBHub transitions AI inference from proprietary platforms to the collective edge. When a query is resolved once by any participant on Earth, its solution is crystallized into the mesh memory. Subsequent identical or semantically close questions are served at sub-5ms latency from local SQLite (L1) or peer swarm gossip (L2) with zero API cost and zero energy waste.

### 1.3 Core Principles
1. **User Sovereignty:** Zero central servers, zero telemetries, zero remote logging, zero user profiling. Data is stored strictly on the local machine in SQLite.
2. **Bring Your Own Key (BYOK):** MBHub acts as a pure protocol layer, not a middleman. Users provide their own official API keys for 10+ cloud providers (OpenAI, Anthropic, Google Gemini, DeepSeek, Groq, OpenRouter, Mistral, Together AI, Perplexity, Cohere, xAI), stored securely under `0600` POSIX permissions.
3. **Atomic Inquiry Discipline ($\le$ 80 characters):** MBHub is an atomic knowledge engine, not a conversational chatbot. Questions are strictly bounded to 80 UTF-8 characters, eliminating prompt bloat, crystallizing technical knowledge, and guaranteeing high-confidence semantic SimHash matching.
4. **Zero-Waste Computing:** Solved computational work is never re-executed.
5. **Local Model Air-Gap:** Inferences generated via local models (Ollama, vLLM, LM Studio, Jan, LocalAI, llama.cpp) are strictly air-gapped (`can_gossip_to_swarm() == false`) and never announced to the mesh.
6. **Strict Wire Constraints:** 64 KB packet ceiling, 1 MB/s upload/download throttling, max 32 concurrent mesh connections (2 concurrent connections per peer).

---

## 2. System Architecture & 3-Tier Pipeline

### 2.1 Procedural Query Flow
When a user submits a query via the TUI, headless CLI, or MCP interface, MBHub executes the following sequence:

```text
               [User Query: Max 80 Characters]
                               │
                               ▼
                 [Pre-Flight DLP & Safety Gate]
                               │
                 ┌─────────────┴─────────────┐
        (Sensitive Data Detected)        (Clean Query)
                 │                           │
                 ▼                           ▼
        [BLOCK: Danger Modal]     [SimHash Normalization]
                                             │
                                             ▼
                                  [Forced Live Query?]
                                     ├── Yes ───────────────────────────┐
                                     └── No (Standard Enter)            │
                                             │                          │
                                             ▼                          │
                                  [L1: Local SQLite Scan]               │
                                             │                          │
                     ┌───────────────────────┴──────────────────────┐   │
                (Cache Hit >= Threshold)                    (Cache Miss)│
                     │                                              │   │
                     ▼                                              ▼   │
           [Instant Terminal Render]                   [L2: P2P Swarm Gossip]
                                                                    │   │
                                             ┌──────────────────────┴───┤
                                        (Swarm Hit)               (Swarm Miss)
                                             │                          │
                                             ▼                          ▼
                                    [Persist to SQLite]       [L3: Live Cloud LLM]
                                             │                          │
                                             ▼                          ▼
                                   [Render to Screen]         [DLP & ANSI Redaction]
                                                                        │
                                                                        ▼
                                                                [Persist to SQLite]
                                                                        │
                                                                        ▼
                                                            [P2P Gossip (L3 Verified)]
```

1. **Normalization & SimHash Generation:** The query is normalized under Unicode NFKC, lowercased, and stripped of punctuation. A 64-bit SimHash fingerprint is computed. SimHash measures semantic similarity as a percentage (0.0%–100.0%) via Hamming distance.
2. **Pre-Flight DLP Gate:** The input is scanned against sensitive credential patterns (API keys, private keys, JWT tokens, credit card numbers). Any match triggers an immediate hard block modal.
3. **Layer 1 (L1) — Local SQLite Scan (0–5 ms):** Scans the local database against the user's configured **Hit Rate Threshold** (70%–99%, default 85%) and **Answer Freshness** policy. On hit, the cached response renders immediately.
4. **Layer 2 (L2) — P2P Swarm Query (up to 600 ms):** If L1 misses, an encrypted query is dispatched over libp2p GossipSub via Noise-encrypted tunnels with 50–300 ms anti-correlation jitter. Verified peer responses under 64 KB are persisted locally and displayed.
5. **Layer 3 (L3) — Live Cloud Model Inference (Streaming):** If L2 misses (or if forced via `Ctrl+Enter`), MBHub establishes a TLS 1.3 streaming connection to the configured provider. The response streams in real-time, passes post-flight redaction, is saved to SQLite, and (if from an authentic cloud provider) is gossiped to the mesh (`is_swarm = false`).

### 2.2 Layout & Wire Budget
* **Query Ceiling:** Strictly 80 UTF-8 characters.
* **TUI Column Budget:** Fits perfectly on standard 110-column terminal displays without horizontal clipping:  
  `DATE [16] + GAP [2] + QUESTION [80] + GAP [2] + HIT (%) [7] = 107 Columns`.
* **Payload Ceiling:** Maximum 64 KB (65,536 bytes) per inference package. Oversized packets are dropped during pre-parse byte streaming.

### 2.3 Local Database Schema (WAL Mode)
```sql
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    val TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS inferences (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp         INTEGER NOT NULL,
    similarity        REAL NOT NULL,
    question          TEXT NOT NULL,
    content           TEXT NOT NULL,
    simhash           INTEGER NOT NULL,
    provider          TEXT NOT NULL,
    model             TEXT NOT NULL,
    content_hash      TEXT,
    is_swarm          INTEGER NOT NULL DEFAULT 0,
    locality          REAL NOT NULL DEFAULT 0.0,
    is_truncated      INTEGER NOT NULL DEFAULT 0,
    publish_candidate INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS profile (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    simhash INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_inferences_ts_sim
    ON inferences (timestamp DESC, similarity DESC);
```

---

## 3. Threat Model & Cybersecurity Hardening

MBHub operates under a **Zero-Trust** security architecture:

| Vector | Attacker Objective | MBHub Defensive Countermeasure | Implementation Location |
| :--- | :--- | :--- | :--- |
| **P2P Eavesdropping / MITM** | Intercept or alter mesh traffic | **Noise Protocol Framework** + Ed25519 static public-key authentication. | `p2p/service.rs`, `p2p/identity.rs` |
| **API Provider MITM** | Intercept or tamper with API keys | TLS 1.3 root CA certificate verification + system trust store. | `api/client.rs` |
| **Eclipse / Sybil Attack** | Isolate and surround victim node | Subnet IP diversity enforcement + random remote peer health checks. | `p2p/service.rs` |
| **Replay Attacks** | Flood identical messages to cause DoS | BLAKE3 content-hash deduplication + timestamp validation ($\pm300$ s) + hop TTL. | `db.rs`, `p2p/protocol.rs` |
| **Gossip Flooding / DoS** | Choke bandwidth with small packets | 20 msgs/sec per peer limit + 64 KB pre-parse ceiling + 1 MB/s bandwidth throttling. | `p2p/service.rs` |
| **Data Tampering** | Mutate answers in transit | **BLAKE3 Content Addressing** + Ed25519 signature. Corrupt payloads dropped on arrival. | `p2p/protocol.rs` |
| **Brand Impersonation** | Masquerade local model as OpenAI | Swarm records strictly tagged `PROVIDER: Unverified (swarm)` and badged `[SWARM]`. | `ui/viewer.rs` |
| **Content Safety / Abuse** | Disseminate harmful/illegal prompts | **Two-phase deterministic + LLM safety gates** (fail-closed, 30 requests/hour budget). | `content_safety.rs` |
| **DLP / Credential Leakage** | Leak API keys or credit cards | Structural regex + Luhn algorithm redaction (`[REDACTED_SECRET]`). | `dlp.rs`, `content_safety.rs` |
| **ANSI Injection** | Execute arbitrary terminal commands | Strict terminal state-machine parser strips OSC 52, CSI cursor escapes. | `sanitize.rs` |
| **Key Theft / File Tampering** | Read private keys or `.env` | Enforce `0600` POSIX permissions; atomic write with temporary files (symlinks not followed). | `p2p/identity.rs`, `env.rs` |
| **Eviction Poisoning** | Force cache thrashing | **Locality Eviction:** In Query Locality mode, lowest-locality records are pruned first. | `db.rs` |
| **De-anonymization / Timing** | Correlate peer IP with query timing | 50–300 ms random query jitter + loose binding between node identity and transport IP. | `app.rs` |

---

## 4. Zero-Friction Unified Installer Architecture

MBHub provides a single unified installer across Linux, macOS, and Windows that sets up the CLI, background daemon, MCP configs, and desktop shortcuts with zero manual steps.

### 4.1 Service Registration Internals (`src/service.rs`)
1. **Linux (`systemd --user`):** Creates `~/.config/systemd/user/mbhub.service` with `ExecStart=<exe> daemon --accept-terms`, enables, and starts immediately via `systemctl --user enable --now mbhub`.
2. **macOS (`launchd`):** Writes `~/Library/LaunchAgents/dev.mbhub.daemon.plist` with `RunAtLoad=true` and `KeepAlive=true`, loading via `launchctl load -w`.
3. **Windows (Task Scheduler):** Registers an auto-starting logon task via `schtasks /Create /SC ONLOGON /TN MBHubDaemon /TR "<exe> daemon --accept-terms" /F`.

### 4.2 Auto-Configured MCP Server
During installation, MBHub injects the stdio MCP configuration into Cursor and Claude Desktop:
```json
{
  "mcpServers": {
    "mbhub": {
      "command": "mbhub",
      "args": ["mcp", "--accept-terms"]
    }
  }
}
```
Target locations:
* Claude Desktop: `~/.config/Claude/claude_desktop_config.json` (Linux) / `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) / `%APPDATA%\Claude\claude_desktop_config.json` (Windows).
* Cursor: `~/.cursor/mcp.json` (Linux/macOS) / `%USERPROFILE%\.cursor\mcp.json` (Windows).

---

## 5. Virtual Windowing & Memory Performance

To guarantee an $O(1)$ memory footprint of **15–25 MB RAM** across databases holding millions of records, MBHub utilizes a 3-tier virtual windowing engine:

1. **Chunked SQLite Paging:** Queries records dynamically using SQL `LIMIT` and `OFFSET`.
2. **Sliding Window Buffer (`WINDOW_SIZE = 150`):** Only a 150-record window surrounding the active cursor is retained in memory. When the cursor scrolls past the boundary, the window slides and out-of-scope entries are freed.
3. **Virtual Viewport Rendering (60+ FPS):** Ratatui only renders rows currently visible within the terminal height (20–30 rows). Frame rendering time remains $<0.1$ ms.

---

## 6. Two-Node Swarm Verification Procedure

To manually test P2P synchronization between two independent nodes on a local machine:

```bash
# Terminal 1 — Node A (Listens on port 45551)
MBHUB_LISTEN_PORT=45551 cargo run

# Terminal 2 — Node B (Listens on port 45552, connects to Node A)
MBHUB_DB=/tmp/mbhub_b.db \
MBHUB_IDENTITY=/tmp/mbhub_b_id.bin \
MBHUB_ENV_FILE=/tmp/mbhub_b.env \
MBHUB_LISTEN_PORT=45552 \
MBHUB_BOOTSTRAP_PEERS=/ip4/127.0.0.1/tcp/45551 \
cargo run
```

Within 3–5 seconds, the top status bar on both nodes displays `PEERS: 1`, and GossipSub inference messages synchronize seamlessly.

---

## 7. Verification & Automated Test Suite

MBHub v1.0.0 is verified with **157 passing automated tests**:

* **Unit Tests:** DLP redaction, ANSI terminal sanitization, BLAKE3 content-hashing, SimHash Hamming distances, Ed25519 identity key generation and repair.
* **Integration Tests:** Pipeline routing (L1 $\rightarrow$ L2 $\rightarrow$ L3), wire integrity gates, anti-poison filters, replay deduplication, and storage quota enforcement.
* **MCP Integration:** Stdio JSON-RPC 2.0 handshake (`initialize`, `tools/list`, `tools/call`, `ping`).
* **P2P Swarm Network Test (`two_swarms_connect_and_gossip_inference`):** Spawns two in-memory libp2p swarms over Noise + Yamux + GossipSub, gossips signed inferences, and verifies end-to-end receipt and database persistence.
