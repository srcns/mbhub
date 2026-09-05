# MBHub v1.0.0 — Sovereign P2P Collective AI Memory

We are proud to announce the initial v1.0.0 private release of **MBHub**, the sovereign, peer-to-peer decentralized inference cache and collective AI memory layer ("The Torrent of Thought").

## What's Included

### 1. Unified Zero-Friction Installation
- **Linux & macOS:** Single-command setup via `curl -fsSL https://mbhub.dev/install.sh | bash`
- **Windows:** Single-command setup via `irm https://mbhub.dev/install.ps1 | iex`
- **Background Daemon Service:** Automatically registers and starts `mbhub` as a background system service (`systemd --user` on Linux, `launchd` on macOS, Task Scheduler on Windows) that auto-starts on login.
- **Model Context Protocol (MCP):** Automatically discovers and configures Cursor (`~/.cursor/mcp.json`) and Claude Desktop (`claude_desktop_config.json`) stdio MCP servers.
- **Desktop Launcher:** Generates application shortcuts (`mbhub.desktop` on Linux, Start Menu on Windows).

### 2. 3-Tier Inference Pipeline
- **L1 (Local SQLite):** Sub-5ms cache hits using 64-bit SimHash Hamming distance similarity.
- **L2 (P2P Mesh):** Libp2p GossipSub over Noise-encrypted tunnels with anti-correlation timing jitter.
- **L3 (BYOK Cloud LLM):** Streaming inference across 10+ providers (OpenAI, Anthropic, Gemini, DeepSeek, Groq, OpenRouter, Mistral, Together, Perplexity, Cohere, xAI).

### 3. Hardened Cybersecurity & DLP
- Pre-flight and post-flight DLP scanning redacting API keys, private keys, JWTs, and credit card numbers (Luhn algorithm).
- BLAKE3 content-addressing and wire integrity validation.
- Strict ANSI terminal escape sanitizer (`sanitize.rs`).
- Mode-conscious storage eviction (Query Locality vs. Blind Swarm).

### 4. Automated Verification
- Full test suite verified: **157 passed; 0 failed**.

## Assets
- `mbhub-linux-x64.tar.gz`
- `mbhub-windows-x64.zip`
- `mbhub-macos-arm64.tar.gz`
- `mbhub-macos-x64.tar.gz`
- `SHA256SUMS.txt` (all four platforms)
