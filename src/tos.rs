//! Terms of Service & Legal Framework for MBHub.
//!
//! Provides the complete, unabridged 17-section legal framework and versioned
//! consent tracking for all peer nodes.

/// Current active version of the MBHub Terms of Service.
/// Incrementing this string automatically triggers the mandatory review
/// and re-acceptance gate for all peer nodes upon their next startup.
pub const CURRENT_TOS_VERSION: &str = "1.0.0";

/// Full, unabridged markdown text of the MBHub Terms of Service & Legal Framework.
pub const TERMS_OF_SERVICE_TEXT: &str = r#"# MBHub Terms of Service & Legal Framework

**Document Version:** 1.0.0  
**Effective Date:** September 5, 2026  
**License:** MIT License (Source Code) + Decentralized Operational Agreement  

---

> **IMPORTANT NOTICE:** MBHub is a sovereign, serverless, peer-to-peer (P2P) collective AI memory network. By connecting to the network, your client participates as an autonomous node. Please review this entire agreement before connecting to the peer-to-peer swarm.

---

### Section 1 — Scope and Acceptance
This agreement governs your use of the MBHub client software ("Client"), the peer-to-peer collective memory network ("Network"), and associated documentation. By initializing the Client, accepting these terms, or maintaining an active connection to the Network, you explicitly accept and agree to be bound by all provisions of this agreement. If you do not agree to these terms in full, you must decline immediately, terminate the process, and refrain from connecting to the Network.

### Section 2 — Definitions
- **Network:** The decentralized, leaderless peer-to-peer collective formed by interconnected Clients communicating over libp2p GossipSub and TCP.
- **Client / Node (Peer):** An autonomous instance of the open-source MBHub software identified by a unique cryptographic PeerID and ed25519 keypair.
- **Content / Record:** An atomic data payload comprising a question, its generated response, 64-bit SimHash coordinates, cryptographic BLAKE3 content-hash, and provenance metadata.
- **Provider:** An external, commercial artificial intelligence inference service (such as OpenAI, Anthropic, Google Gemini, DeepSeek, Groq, or OpenRouter).
- **BYOK (Bring Your Own Key):** The sovereign operational model where the node operator provides and manages their own independent provider credentials.

### Section 3 — Nature of the Service
MBHub is free, open-source, serverless software. There are no central servers, no user accounts, no administrative backdoors, and no corporate controllers. The Network is an emergent collective of autonomous peer nodes. No single entity or developer controls, operates, or assumes comprehensive liability for the Network. Network capacity, response latency, and coverage depend entirely on participating peers at any given moment; no uptime, answer completeness, or accuracy SLA is guaranteed.

### Section 4 — Open Source Licensing & Operational Framework
The MBHub Client source code is released under the permissive MIT License (see project repository). This document serves as an operational agreement outlining acceptable network rules, community defense obligations, content safety boundaries, and liability limitations not covered by the copyright license.

### Section 5 — User & Node Operator Obligations
- When connecting to an AI Provider via BYOK, you are solely and exclusively responsible for complying with that Provider's respective Terms of Service and acceptable use policies. MBHub is not a party to that commercial relationship.
- You agree to operate your node in full compliance with all applicable local, national, and international laws and regulations.
- You are solely responsible for the security and confidentiality of your local private keys, API credentials, and database files. Sensitive credentials are protected client-side via Data Loss Prevention (DLP) and are never transmitted over the P2P wire.

### Section 6 — Prohibited Uses & Material Breaches
The following actions constitute a material breach of this agreement and will trigger autonomous peer rejection:
1. Ingesting, generating, broadcasting, or soliciting content within illegal categories, including but not limited to child sexual abuse material (CSAM), violent extremism, weapons of mass destruction, or unlawful malicious cyberweapons.
2. Tampering with, circumventing, or disabling client-side content safety screening, DLP regex engines, BLAKE3 wire integrity gates, or Anti-Poison filters.
3. Conducting Sybil attacks, automated socket flooding, denial-of-service attempts, or coordinated falsification of cryptographic negative signals (Tombstones).
4. Attempting to gain unauthorized access to peer machines, private key files, or local SQLite shards.
5. Modifying the Client to poison the collective memory with empty, truncated, or deliberately malformed payloads.

### Section 7 — Content Integrity & Autonomous Network Regulation
Every response circulated across the Network is authenticated by a canonical BLAKE3 content hash. Peer nodes independently evaluate and verify incoming data before admitting records to local storage. Inaccurate, stale, or poisoned content can be challenged through decentralized negative signals (Tombstones), while malicious payloads are rejected client-side by immutable fail-closed gates.

### Section 8 — Privacy & Zero Telemetry Guarantee
MBHub collects zero telemetry, zero analytics, zero user metrics, and zero crash reports. No telemetry packets are ever transmitted. All personal query history remains confined to your private local SQLite database. Network queries carry only ephemeral cryptographic peer identifiers. When communicating directly with third-party AI providers in L3 mode, that provider's privacy policy applies exclusively.

### Section 9 — Disclaimer of Warranties
THE CLIENT AND NETWORK ARE PROVIDED "AS IS" AND "AS AVAILABLE", WITHOUT WARRANTIES OF ANY KIND, EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, AND NON-INFRINGEMENT. THE PROJECT MAKES NO WARRANTY THAT ANSWERS RETRIEVED FROM PEERS ARE ACCURATE, COMPLETE, OR VERIFIED. YOU RELY ON PEER-SERVED RESPONSES AT YOUR OWN DISCRETION AND RISK.

### Section 10 — Limitation of Liability
TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, IN NO EVENT SHALL THE AUTHORS, MAINTAINERS, OR CONTRIBUTORS OF MBHUB BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, CONSEQUENTIAL, OR PUNITIVE DAMAGES ARISING OUT OF THE USE OF OR INABILITY TO USE THE CLIENT OR NETWORK, OR ANY DATA RETRIEVED THEREFROM.

### Section 11 — Indemnification
You agree to indemnify, defend, and hold harmless the authors, maintainers, and contributors of MBHub against any claims, liabilities, damages, judgments, or expenses arising out of your violation of this agreement or your unlawful operation of a network node.

### Section 12 — Compliance & Sovereign Operator Responsibility
The Client contains local hard gates engineered to reject unlawful materials. Because the Network operates without central servers, legal compliance responsibilities rest strictly with individual participants within their respective sovereign jurisdictions.

### Section 13 — Autonomous Enforcement & Termination
Traditional account suspension does not exist in a serverless peer-to-peer network. Non-compliant nodes exhibiting malicious behavior, protocol tampering, or poisoned broadcasts are autonomously detected and dropped by peer clients. You may terminate your participation at any time by stopping the software and deleting your local database.

### Section 14 — Protocol Amendments & Versioned Consent
These terms may be amended to reflect protocol upgrades and technical developments. Whenever a material amendment is made, the document version will be updated, and the Client will automatically prompt you for renewed confirmation upon launch before restoring swarm connectivity.

### Section 15 — Governing Law & Dispute Resolution
This agreement shall be governed by and construed in accordance with the laws of the Republic of Turkey. Any legal disputes arising out of or related to this agreement shall be subject to the exclusive jurisdiction of the competent courts of Istanbul (Çağlayan).

### Section 16 — Severability
If any provision of this agreement is determined to be unlawful, void, or unenforceable, that provision shall be deemed severable and shall not affect the validity and enforceability of any remaining provisions.

### Section 17 — Contact & Open Source Verification
For inquiries, legal notices, audits, or contributions, refer to the official public GitHub repository:
https://github.com/srcns/mbhub

---

**ACCEPTANCE COMMANDS:**
- Press **[ Enter ]** or **[ Y ]** to accept this agreement and connect to the P2P swarm.
- Press **[ Esc ]** or **[ Q ]** to decline and exit the application immediately.
- Use **[ ↑ / ↓ / PageUp / PageDown ]** to scroll through the full document.
"#;
