# 1 — Create the phase

> **Run once already** for phase `mailcraft-ui-refresh-and-agent-runtimes`: its gate answers, final change ids and frozen scenario ids are in [README.md](README.md#the-record-of-the-phase-these-prompts-ran). OpenCode is cut (G-RT5 b).

Inputs: the phase chosen in prompt 0 (`<PHASE>`, `<PART>`) and the start commit hash.

1. Read `docs/design/PLAN.md`. For Part A, read §4. For Part B, read §B4.
2. Run `/kbd-new-phase <PHASE>` with the goals listed in that section, copied verbatim. They are already written as KBD goals.
3. In the phase's `prior-context.md` (in the KBD state folder), record:
   - the start commit hash;
   - links to `docs/design/PLAN.md`, the prototypes, `AI-SETTINGS-SPEC.md` and `research/AGENT-RUNTIMES.md` (Part B);
   - the published review records in `docs/design/review/`;
   - one line: "The plan in docs/design is a proposal; gates are unanswered until decision-log.md records them."
4. Confirm `position-reminder.txt` points at `<PHASE>` / assess. Stop.
