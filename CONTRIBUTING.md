# Contributing to MBHub

Thank you for considering a contribution. MBHub is **Source Available**
(PolyForm Noncommercial 1.0.0) and every contribution helps keep the
collective memory honest, fast, and safe.

## Before you start

1. **CLA (Contributor License Agreement).** Before your first pull request is
   merged you must sign the CLA. A bot will comment on your PR with a
   one-click signing link; until it is signed, the PR cannot be merged.
   Why: contributions must be licensable under both the non-commercial and
   the commercial terms of the project — the CLA gives the project the
   perpetual right to use, modify, and relicence your contribution.
2. **Keep the scope sovereign.** No features that centralize the data plane,
   add telemetry, or weaken the client-side safety gates.
3. **English only** in all user-facing strings, logs, docs, and commit messages.

## How to contribute

1. Fork the repository and create a feature branch.
2. Write tests for any behavior you add or change — the suites must stay green:
   ```bash
   cargo test                          # public/default profile
   cargo test --features publisher     # maintainer profile
   ```
3. Run `cargo fmt` and `cargo clippy`.
4. Open a pull request with a clear description of the *why*, not just the *what*.
5. Sign the CLA when the bot asks (one click, stored forever).

## Code of conduct

Be precise, be kind, and attack systems — never people. Security-relevant
findings should **not** be opened as public issues; contact the maintainers
directly through the repository's security advisory channel.
