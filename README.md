<p align="center">
  <img src="logo-black.svg#gh-light-mode-only" alt="MBHub Logo" width="120">
  <img src="logo-white.svg#gh-dark-mode-only" alt="MBHub Logo" width="120">
</p>

<h1 align="center">MBHub — Sovereign P2P Collective AI Memory</h1>

<p align="center">
  <strong>The Torrent of Thought: A serverless, decentralized inference cache and collaborative memory layer for AI.</strong>
</p>

<p align="center">
  <a href="https://github.com/srcns/mbhub/actions"><img src="https://img.shields.io/badge/tests-150%20passed-brightgreen.svg" alt="Tests"></a>
  <a href="https://github.com/srcns/mbhub"><img src="https://img.shields.io/badge/rust-2024%20edition-orange.svg" alt="Rust Edition"></a>
  <a href="https://github.com/srcns/mbhub"><img src="https://img.shields.io/badge/p2p-libp2p%20%2B%20noise-blue.svg" alt="P2P Protocol"></a>
  <a href="https://github.com/srcns/mbhub"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="License"></a>
  <a href="https://mbhub.dev"><img src="https://img.shields.io/badge/web-mbhub.dev-26A269.svg" alt="Website"></a>
</p>

---

## 1. The Vision: The Torrent of Thought

Modern AI computing is centralized in monolithic server farms. Every second, millions of developers and users ask nearly identical technical questions:
- *"How does the ownership model work in Rust?"*
- *"How do I implement distributed consensus using Raft?"*
- *"What are client-side cybersecurity best practices in P2P networks?"*

For every query, massive GPU clusters execute redundant tensor multiplications, consuming megawatts of electricity and charging users per token.

**MBHub fundamentally changes this paradigm.** Just as BitTorrent liberated file distribution from centralized web servers by pooling peer bandwidth, MBHub liberates AI inference by creating a sovereign, decentralized, collective memory network.

When a query is resolved anywhere on the planet, its verified answer is crystallized into the collective memory mesh. The next time anyone needs that knowledge, it is served in sub-5ms from local memory (L1) or peer swarm gossip (L2) with **zero latency, zero API costs, and zero redundant energy waste**.

---

## 2. Core Architectural Tenets

1. **Absolute Sovereignty:** Zero central servers, zero telemetries, zero tracking, and zero surveillance. Your knowledge, logs, and cryptographic keys reside entirely on your local machine in SQLite.
2. **Bring Your Own Key (BYOK):** MBHub is not a proxy broker. Configure your own official API keys across 10+ providers (OpenAI, Anthropic Claude, Google Gemini, DeepSeek, Groq, OpenRouter, Mistral, Together AI, Perplexity, Cohere, xAI).
3. **Atomic Inquiry Discipline ($\le$ 80 characters):** MBHub enforces an 80-character maximum on questions. This eliminates prompt bloat, crystallizes atomic technical knowledge, and guarantees high-confidence semantic SimHash matching.
4. **Local Model Air-Gap:** Inferences generated via local models (Ollama, vLLM, LM Studio, Jan, LocalAI, llama.cpp) are air-gapped (`can_gossip_to_swarm() == false`) and never leak to the public mesh.
5. **Zero-Waste Computing:** Solved intelligence is never recomputed from scratch.
6. **Strict Wire Constraints:** 64 KB packet ceiling, 1 MB/s upload/download throttling, max 32 concurrent mesh connections.

---

## 3. Zero-Friction One-Step Installation

MBHub ships as a self-contained, statically-linked standalone binary written in Rust. It requires no external dependencies (no Python, no Node.js runtime) and uses only 15–25 MB of RAM.

### macOS & Linux (Apple Silicon & Intel / x86_64 & ARM64)
```bash
curl -fsSL https://mbhub.dev/install.sh | bash
```

### Windows (PowerShell 5.1+ & 7+)
```powershell
irm https://mbhub.dev/install.ps1 | iex
```

### What does the unified installer do automatically?
1. **Binary Deployment:** Installs `mbhub` into your user PATH (`~/.local/bin/mbhub` or `%LOCALAPPDATA%\Programs\MBHub\mbhub.exe`).
2. **Background Daemon:** Registers and launches the 24/7 background peer node as a system service (`systemd --user` on Linux, `launchd` on macOS, Scheduled Task on Windows) that automatically boots on system login.
3. **Automatic MCP Integration:** Automatically injects the `mbhub` stdio server into Cursor (`~/.cursor/mcp.json`) and Claude Desktop (`claude_desktop_config.json`).
4. **Desktop Launcher:** Generates application shortcuts (`mbhub.desktop` or Start Menu item).

---

## 4. 3-Tier Inference Pipeline

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

* **L1 — Local SQLite Cache (0–5 ms):** Scans local indexed records using 64-bit SimHash Hamming distance coordinates against your configured Hit Rate Threshold (70%–99%, default 85%).
* **L2 — P2P Mesh Swarm (up to 2.5 s):** Dispatches encrypted query over libp2p GossipSub to adjacent peers with 50–300 ms anti-correlation jitter. Peer discovery runs on a **Kademlia DHT** (`/mbhub/kad/1.0.0`): every reachable peer is an introducer, so only the very first contact needs a known address. NAT traversal is automatic (AutoNAT probes, UPnP port mapping, DCUtR hole punching, and circuit-relay v2 fallback), and mDNS discovers peers on the local network for free. A node with zero peers skips L2 instantly, so the pipeline never stalls.
* **L3 — Live Cloud Model (Streaming):** Queries provider API via TLS 1.3, streams tokens in real-time, redacts credentials, saves to SQLite, and announces verified inference to the swarm.

---

## 5. CLI & Daemon Command Reference

The single `mbhub` executable manages interactive exploration, headless queries, daemon services, and MCP servers:

| Command | Description |
| :--- | :--- |
| `mbhub` | Launches the interactive terminal user interface (TUI). |
| `mbhub ask "<query>" [--json]` | Executes a sub-5ms headless query via the 3-tier pipeline directly in scripts or shell. |
| `mbhub daemon [--accept-terms]` | Runs the 24/7 headless background node (P2P mesh + IPC server). |
| `mbhub bootstrap` | Runs a dedicated rendezvous node (Kademlia DHT server + circuit-relay v2 server) for community-hosted bootstrap instances. Carries no user content and no database. |
| `mbhub mcp [--accept-terms]` | Starts the standard JSON-RPC 2.0 stdio MCP server for Cursor and Claude Desktop. |
| `mbhub status` | Checks the live operational health of the background service, peer count, and storage. |
| `mbhub service install` | Installs the background service, MCP configurations, and desktop shortcuts. |
| `mbhub service start \| stop` | Starts or stops the background daemon service. |
| `mbhub service uninstall` | Uninstalls and disables the background service. |
| `mbhub update [--check]` | Performs seamless, in-place binary upgrades without database loss. |

---

## 6. Interactive Terminal UI (TUI) Cheatsheet

Navigate between screens using `Tab`:

### Screen 1: ASK
* Type your question ($\le$ 80 chars).
* `Enter`: Resolve via 3-Tier Pipeline (L1 Cache $\rightarrow$ L2 Swarm $\rightarrow$ L3 Cloud Provider).
* `Ctrl+Enter`: Force L3 live model query, bypassing local and swarm cache.
* `Esc`: Close markdown response viewer and restore query input.

### Screen 2: MEMORY
* `↑` / `↓`: Scroll through cached inference records.
* `Enter`: Open full markdown response viewer.
* `d` / `Delete`: Delete a record locally and broadcast a P2P tombstone so peers never serve it again.

### Screen 3: SETTINGS
* `↑` / `↓`: Navigate flat settings list.
* `←` / `→` or `Enter`: Cycle values or open picker modals:
  * **Reserved Storage:** Set local storage cap (1 GB default).
  * **Sharding Mode:** Cycle between **Query Locality** (evicts least relevant records) and **Blind Swarm** (evicts oldest).
  * **Hit Rate Threshold:** Set similarity cutoff (70% – 99%).
  * **Provider & Model:** Choose from 10+ AI providers and customize models.
  * **API Keys:** Securely stored with `0600` permissions.

---

## 7. Model Context Protocol (MCP) Integration

MBHub features native stdio MCP server support. Once installed, AI coding assistants query MBHub's collective memory before making external API requests.

### Configuration (`~/.cursor/mcp.json` or `claude_desktop_config.json`)
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

### Exposed MCP Tools
* `mbhub_ask(query: string)`: Searches L1 SQLite and L2 swarm for instant, verified answers.
* `mbhub_status()`: Reports node health, peer count, and storage quota.

---

## 8. Peer Discovery & Network Bootstrap

MBHub nodes find each other through a **Kademlia DHT** — no central server is involved in content delivery, and the network itself does the introductions:

1. **First contact:** a fresh node dials a small, hard-capped list of rendezvous addresses resolved in this priority order:
   * `MBHUB_BOOTSTRAP_PEERS` (comma-separated multiaddrs — operator override),
   * the embedded default list shipped with the binary,
   * `https://mbhub.dev/bootstrap.json` (fetched fresh, TLS-only, size-capped, with a 30-minute refresh),
   * `~/.mbhub/bootstrap-cache.json` (owner-only cache from the last successful fetch — offline resilience).
2. **Introductions:** after the DHT bootstrap query completes, every peer's `identify`-exchanged addresses enter the requester's routing table — each reachable node becomes an introducer, and the bootstrap rendezvous drops out of the loop entirely.
3. **Traversal:** AutoNAT verifies reachability; UPnP maps ports where the gateway allows; DCUtR punches holes through NATs; circuit-relay v2 (via the bootstrap/community nodes) is the last-resort transport for hard-NAT'd peers.
4. **Local network:** mDNS discovers peers on the same LAN instantly, without any address configuration.
5. **Listening:** nodes listen on the well-known TCP port **37777** by default (`MBHUB_LISTEN_PORT` overrides; a busy port falls back to an ephemeral one so a second local instance still runs).

Running a rendezvous node: `mbhub bootstrap` starts a hardened Kademlia server + relay server that helps peers meet but never stores or relays user content (strict reservation/circuit caps, no database).

---

## 9. Cybersecurity & Threat Model

MBHub is built on a zero-trust architecture hardened against malicious actors:

| Vector | Threat Target | MBHub Mitigation |
| :--- | :--- | :--- |
| **MITM / Eavesdropping** | Intercepting P2P traffic | **Noise Protocol Framework** + Ed25519 static public-key authentication (`p2p/service.rs`). |
| **Tampering** | Modifying swarm answers | **BLAKE3 Content Addressing** + Ed25519 creator signature. Corrupt payloads dropped on arrival. |
| **Credential Leakage** | API keys or cards in prompt | **Pre-flight & Post-flight DLP scanner** with regex and Luhn algorithm redaction (`dlp.rs`). |
| **Terminal Hijack** | Malicious ANSI escape codes | **Strict State-Machine Sanitizer** strips OSC 52, CSI cursor control, and terminal escapes (`sanitize.rs`); CLI output is sanitized at the source. |
| **Swarm Censorship** | Forged deletion (tombstone) broadcasts | **Ed25519-signed tombstones:** unsigned, mis-signed, or mis-attributed negative signals are dropped at the swarm edge (`p2p/service.rs`). |
| **Brand Spoofing** | Masquerading fake outputs | Swarm-sourced records strictly labeled `PROVIDER: Unverified (swarm)` (`ui/viewer.rs`). |
| **DoS / Flooding** | Message flooding attacks | Peer rate-limiting (20 msgs/sec), 64 KB pre-parse ceiling, 1 MB/s bandwidth cap. |
| **Eviction Poisoning** | Forcing cache thrashing | **Locality Eviction:** In Query Locality mode, records least relevant to user profile are pruned first. |

---

## 9. Verification & Automated Tests

MBHub maintains an exhaustive test suite verifying every component from protocol wire integrity to UI boundary conditions:

```bash
cargo test
```
```text
test result: ok. 169 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.83s
```

---

## 10. Documentation Suite

* [Master Technical Specification](docs/SPECIFICATION.md) — Comprehensive architectural, wire, and algorithmic specification.
* [Official Knowledge Commons](https://mbhub.dev) — Browse live, verified Q&A records.

---

<p align="center">
  <strong>MBHub: Consumes little, wastes nothing, never forgets, shares freely, and unconditionally respects user sovereignty.</strong>
</p>
