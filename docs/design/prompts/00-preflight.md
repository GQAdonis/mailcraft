# 0 — Preflight and phase order

You are the KBD lead for MailCraft (Rust, egui). Don't write code in this session.

1. Read, in this order:
   - `AGENTS.md`
   - `.kbd-orchestrator/constraints.md`
   - `.kbd-orchestrator/position-reminder.txt`
   - `docs/design/PLAN.md` (§0, §4 and Part B §B0–B1)
2. **Check the entry criteria and report each as met or not met, with evidence:**
   - `agentic-ai-integration` and its child `agentic-ai-scenario-repair` have closed. Read `progress.json` and `decision-log.md` for both.
   - The AI code is committed. Run `git status --short crates/ai crates/ui-egui/src/ai crates/engine/src/cmd/ai apps/mailcraft/src/ai` (it should be empty) and `git log -1`.
   - Child tasks 6.4, 6.6 and 7.2–7.6 are done in the child's `tasks.md`.
   - `cargo xtask ci` is green on HEAD. Run it, and use a fresh `CARGO_TARGET_DIR`.
3. If any criterion is unmet, **stop**. Report what is missing and which phase or task owns it. Don't start new phases around an unfinished one.
4. If all are met, ask the owner one question: **G-RT0, which phase goes first**.
   - Recommendation: Part A (`window-refresh-and-agent-console`), then Part B (`agent-runtimes`). Reason: PLAN.md §B0.3.
   - Use a short question form, with the recommendation pre-selected.
5. Record the answer for the next prompt: phase name, part, and the commit hash the phase starts from.
