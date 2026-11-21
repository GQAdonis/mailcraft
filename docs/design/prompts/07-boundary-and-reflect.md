# 7 — Boundary, reviews, acceptance and reflect

> **Run once already** for phase `mailcraft-ui-refresh-and-agent-runtimes`: its gate answers, final change ids and frozen scenario ids are in [README.md](README.md#the-record-of-the-phase-these-prompts-ran). OpenCode is cut (G-RT5 b).

For `<PHASE>`, after the last wave:

1. **Boundary run.** Run every frozen scenario (43 for this phase, README.md). Live scenarios run on the owner's own logins with synthetic mail; a provider error or an exhausted plan quota is BLOCKED, never FAIL. Deterministic scenarios run first; live ones (for example SCN-RT-PRIV-001 packet capture and SCN-RT-LIVE-001) are owner-gated: ask before running anything that needs sudo or the owner's own credentials. Record dated evidence in `boundary/`.
2. **Reviews.** Wake `security-privacy-reviewer` and `rust-quality-reviewer`, read-only, once, then apply one batched correction cycle. For Part B, point security review at B5.4 (`ai-runtime-containment`), B5.5 and B5.6 (`ai-runtime-claude-code`, `ai-runtime-codex`), B5.7 (`ai-runtime-chatgpt-siwc`) and B5.9 (`ai-runtime-uar`). The review range is everything since the commit before the phase.
3. **Visual evidence.**
   - Part A: PLAN.md §5.9.
   - Part B: §B9.
   - Look at each PNG yourself.
4. **Gate.** `cargo xtask ci` (including wasm) and `openspec validate --strict` must both pass. Archive the changes.
5. **Owner acceptance.** Show the owner the scenario table, the screenshots and the residual risks. Wait for an explicit yes.
6. **Commit, only after the yes.**
   - Commit by area.
   - Write the messages in conventional style, ending with `Human-reviewed, AI-assisted, nobody's mailbox harmed.`
   - No AI co-author trailer.
   - Never commit the private planning folders (plans, OpenSpec changes, KBD state).
   - Add `docs/log.d` and `packaging/release-notes.d` fragments.
7. **Reflect.** Run `/kbd-reflect`: what held, what drifted, and lessons for the next phase. For Part A, recommend Part B as the next phase, or whatever G-RT0 decided.
