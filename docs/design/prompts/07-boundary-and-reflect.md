# 7 — Boundary, reviews, acceptance and reflect

For `<PHASE>`, after the last wave:

1. **Boundary run.** Run every frozen scenario. Deterministic scenarios run first; live ones (for example SCN-RT-PRIV-001 packet capture and SCN-RT-LIVE-001) are owner-gated: ask before running anything that needs sudo or the owner's own credentials. Record dated evidence in `boundary/`.
2. **Reviews.** Wake `security-privacy-reviewer` and `rust-quality-reviewer`, read-only, once, then apply one batched correction cycle. For Part B, point security review at B5.4 (containment), B5.7 (OAuth) and B5.9 (UAR).
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
   - Never commit `plan/`, `openspec/` or `.kbd-orchestrator/`.
   - Add `docs/log.d` and `packaging/release-notes.d` fragments.
7. **Reflect.** Run `/kbd-reflect`: what held, what drifted, and lessons for the next phase. For Part A, recommend Part B as the next phase, or whatever G-RT0 decided.
