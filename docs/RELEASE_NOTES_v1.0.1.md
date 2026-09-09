# MBHub v1.0.1 — Signed Wire Protocol & Security Hardening

v1.0.1 turns the wire protocol end-to-end attributable and closes every
HIGH-severity finding from the September 2026 deep security audit. All local
data, databases, and published records remain fully compatible — no migration
or reconfiguration is required. (Network cutover note: content from peers on
older builds is unsigned and is no longer accepted.)

## Wire Protocol — Mandatory Attribution

- **Inference records** require an Ed25519 author signature covering every
  integrity-relevant field (question, content, provider, model, timestamp,
  simhash, content_hash). Receivers verify against the authenticated gossip
  author before anything is stored or displayed — poisoning becomes
  attributable, ban-able evidence.
- **Swarm queries** require the asker's signature — query spam is attributable.
- **Query responses** require the responder's signature AND a 16-bit
  proof-of-work over the content hash — the query-response channel is no
  longer a zero-cost content injection route, and nodes re-serve only
  self-produced (PoW'd) content.
- Signing and PoW solving are centralized in the swarm loop; receiving edges
  verify before any channel, database, or UI touch.

## Security Hardening (audit remediation)

- **MCP framing:** swarm answers returned to AI assistants (Cursor, Claude
  Desktop) are wrapped in an UNVERIFIED banner, explicit BEGIN/END delimiters,
  and capped at 16 KB — indirect prompt injection defense.
- **Canonical store:** API keys and the SQLite database live exclusively in
  the owner-only `~/.mbhub/` directory; a repo-planted `.env`/`mbhub.db` can
  no longer hijack reads or leak keys through working-directory writes.
- **DLP:** modern key shapes added (OpenAI `sk-proj-`, Google `AIza`, GitHub
  `ghp_`/`github_pat_`, Groq `gsk_`, Perplexity `pplx-`, xAI, Slack, Stripe).
- **Remote DoS closed:** swarm queries answer from a bounded 500-record
  recent window (no more full-table scans on the swarm thread), and a
  node-wide 120 msg/s aggregate cap layers on top of the 20 msg/s per-author
  limiter (Sybil multiplication neutralized).
- **SimHash matching hardened:** questions under 12 characters require an
  exact match; the substantive-word guard now applies at every length —
  short-question cache poisoning is structurally impossible.
- **DHT hygiene:** identify-supplied addresses enter the Kademlia routing
  table only when globally routable (client + rendezvous nodes).
- **Storage:** the daemon enforces the quota per drain pass; headless/CLI/MCP
  queries are capped at 512 bytes and trigger eviction; database insert
  failures degrade gracefully instead of panicking.
- **Backup restore:** the live `meta` table (credentials) always survives a
  restore, and restored rows pass DLP/content-safety gates before serving.
- **Installers:** local-package override requires `./SHA256SUMS.txt` or an
  explicit `MBHUB_ALLOW_LOCAL=1` — a fork-planted binary can no longer
  silently become the trust root. Windows self-update gained rollback.
- **MCP config merge** aborts on unparseable files instead of destroying the
  user's other servers; writes are atomic.

## Auto-Update Flow

The background daemon checks the public release manifest every 6 hours,
applies the verified update atomically (mandatory SHA-256 manifest), and
restarts through the system supervisor. Opt out with `MBHUB_AUTO_UPDATE=0`.
Publisher builds never self-update.

## SEO & Distribution

- IndexNow pipeline: every deploy pings Bing/Yandex/Seznam with the changed
  URL set (47/47 accepted on release day).
- Branded 1200×630 OG card wired site-wide; robots.txt added; archive
  redesigned as a compact, build-time-paginated list that scales to tens of
  thousands of records.

## Verification

- 196 automated tests passing (signature gates, PoW gates, rate-limit
  semantics, restore gates, installer gates), including an adversarial
  in-process swarm rehearsal covering 9 attack scenarios.

## Assets

- `mbhub-linux-x64.tar.gz` · `mbhub-macos-arm64.tar.gz` ·
  `mbhub-macos-x64.tar.gz` · `mbhub-windows-x64.zip` · `SHA256SUMS.txt`

## Upgrade

```bash
mbhub update
```
or reinstall with `curl -fsSL https://mbhub.dev/install.sh | bash`.
