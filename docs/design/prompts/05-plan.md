# 5 — Plan

Run `/kbd-plan` for `<PHASE>`. The draft change list is already written:

- **Part A:** `docs/design/PLAN.md` §5 (changes 5.1–5.9) and §6 (waves and grants).
- **Part B:** `docs/design/PLAN.md` §B5 (B5.1–B5.13) and §B6. `docs/design/AI-SETTINGS-SPEC.md` is the normative spec, as amended by the gate answers (B5.8 OpenCode cut).

*Recorded result:* 21 changes and 43 frozen scenario ids, listed in [README.md](README.md#the-record-of-the-phase-these-prompts-ran).

Produce:

1. **One OpenSpec change per row.** Each gets `proposal.md`, `design.md`, `tasks.md` and `specs/<capability>/spec.md`, and must pass `openspec validate --strict`. Adjust each change for the gate answers in `decision-log.md`; a gate answered against the recommendation can remove or add a change.
2. **`plan.md`** in the house format:
   - Inputs
   - Owner decisions
   - Recalled lessons
   - Goal→change map
   - Changes in order
   - Scenario→task coverage
   - Task model assignments (`Phase path | Change | Tasks | Role (route) | Why this model`)
   - Budgets
   - Boundary packet
   - Unresolved findings
3. **Frozen scenario ids in the adopted contract:**
   - Part A: PLAN.md §7.
   - Part B: spec §13.
4. **`execution.md` waves with explicit file-ownership grants.**
   - `lib.rs` and the adapter registry are integrator-owned.
   - `agent.rs` belongs to engine-engineer.
   - `theme*` belongs to Part A's 5.1.
   - `settings.rs` v3 belongs to B5.1 (`ai-runtime-model`).
5. **Adversarial review of `plan.md`.** Cross-model, at most 2 rounds. Any CRITICAL finding must be fixed or recorded under "Unresolved review findings".

Get the owner's sign-off on the adopted contract, then stop.
