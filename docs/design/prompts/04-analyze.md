# 4 — Analyze

Run `/kbd-analyze` for `<PHASE>`.

Look for candidates to adopt before building anything. Respect `deny.toml` (licences) and AGENTS.md (Rust only, no Node in the app, no new I/O in the engine):

- **Part A:** egui_kittest helpers for type-floor checks; nothing else is expected.
- **Part B:**
  - Rust OAuth 2.0 PKCE crates. Compare them against writing about 200 lines on top of `crates/net`: check licence, size, TLS stack, and whether they fit `crates/net`'s loopback and TLS rules.
  - Process-group crates (`command-group` or equivalent) vs a small hand-written `setsid`/`killpg` + Windows job object.
  - The `agent-client-protocol` crate, for a **future** ACP adapter only. Record it as `defer`.
  - JSON-lines parsing: use the existing `serde_json` with the caps that are already in place.

Write `library-candidates.json` with verdicts `adopt` / `adapt` / `build` / `defer`, and evidence for each. Run an adversarial review of `analysis.md` (at most 2 rounds), then stop.
