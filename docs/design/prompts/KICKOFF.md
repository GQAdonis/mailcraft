# Kickoff: one phase for all of `docs/design`

Paste everything inside the fence into Claude Code (Opus 5.5), started in the MailCraft repo root. It creates **one** KBD phase that delivers Part A and Part B of `PLAN.md` from the repository as it is now. Prompts `02`–`07` stay the detailed instructions for each stage.

> **Run.** This kickoff created the phase `mailcraft-ui-refresh-and-agent-runtimes`. Its gate answers, its 21 final change ids and its 43 frozen scenario ids are in [README.md](README.md#the-record-of-the-phase-these-prompts-ran), and `AI-SETTINGS-SPEC.md` carries the amendments. The text in the fence is kept as it was pasted, except the rules that the gate answers changed (marked **Amended**). OpenCode is cut (G-RT5 b).

```text
/kbd-new-phase mailcraft-ui-refresh-and-agent-runtimes Deliver everything in docs/design: the window refresh, AI and agent surfaces (PLAN.md Part A) and the multi-runtime AI settings and sign-in (PLAN.md Part B), starting from the repository as it is now.

You are the KBD lead for MailCraft, running Opus 5.5. Run the full loop for this one phase: assess -> analyze -> plan -> execute -> reflect. Stop at each owner checkpoint below.

## Sources of truth (read in this order before anything else)
1. AGENTS.md. It is the authority. If anything in docs/design conflicts with it, AGENTS.md wins and you raise the conflict with me.
2. The KBD constraints file and position reminder in the KBD state folder.
3. docs/design/PLAN.md: all of Part A (§0-§11) and Part B (§B0-§B11).
4. docs/design/AI-SETTINGS-SPEC.md: normative for Part B (config v3, detection, sign-in, containment, commands, scenarios SCN-RT-*).
5. docs/design/research/AGENT-RUNTIMES.md: vendor facts, policy quotes and the verification list.
6. docs/design/DESIGN-PROPOSAL.md and the prototypes: index.html (main window) and settings.html (AI settings). They show intent; PLAN.md's gates decide what is built.
7. docs/design/prompts/README.md and prompts 02-07: the step-by-step instructions for each stage. Follow them, with <PHASE> = mailcraft-ui-refresh-and-agent-runtimes and both parts in scope.
8. docs/design/review/: earlier adversarial findings. Don't reopen resolved ones without new evidence.

## Starting point: the repository as it is now
- The earlier phase agentic-ai-integration (and its child agentic-ai-scenario-repair) is marked cancelled in KBD state. PLAN.md §4 and §B1 assumed it would close normally, so treat those entry criteria as superseded by this baseline step, not as met.
- The working tree has uncommitted changes, including AI code PLAN.md builds on (ai.json v2, route.rs, settings_page.rs, settings_device.rs). Never discard, stash, reset or overwrite them.
- Baseline step, before assess:
  a. Record git log -1, git status --short and the KBD state in this phase's prior-context.md in the KBD state folder, with links to every docs/design file above and the line "The plan in docs/design is a proposal; gates are unanswered until decision-log.md records them."
  b. Run cargo xtask ci on the current tree with a fresh CARGO_TARGET_DIR and record the result.
  c. List which items PLAN.md §1 and §B2 say exist, and check each one in code: present, partial or missing. Pay special attention to the child-phase tasks PLAN.md depends on (6.4, 6.6, 7.2-7.6).
  d. Ask me one question: how to baseline the uncommitted work. Recommend: I commit it myself as a separate baseline commit on a new branch before execute. Any missing dependency from (c) becomes a change in this phase's plan; don't assume it.

## Phase shape
- One phase, two tracks:
  - Track A = PLAN.md §5.1-5.9.
  - Track B = PLAN.md B5.1-B5.13.
  - Plus any baseline gap changes from step (c).
- goals.md = the goals in PLAN.md §4 and §B4, copied verbatim, plus one goal: "Close the gaps the baseline found in the earlier AI work."
- Out of scope: the union of the cuts in §4 and §B4.
- Order is gate G-RT0. Default: Track A waves first, then Track B. Apply §B10: B5.10 and B5.11 update A's status chip, provenance and narrowing badge, so A must not hard-code what B changes.
- Waves, ownership and model routing come from PLAN.md §6 and §B6, including the W2/W3 entry gates. Use the project's agents (ui-engineer, engine-engineer, ai-provider-engineer, agent-surface-engineer, architect, ui-ux-designer, security-privacy-reviewer, rust-quality-reviewer).

## Stage by stage
- ASSESS (prompt 02):
  - Re-verify the code inventory.
  - Run the verification list in research/AGENT-RUNTIMES.md against current vendor docs, with sources.
  - Then present ALL open gates in one question form, each with its recommendation pre-selected: G1-G18 and G-RT0-G-RT13 (prompt 03). G-RT2, G-RT3 and G-RT12 need my answer in my own words; don't pre-fill them.
  - Record every answer in decision-log.md.
  - CHECKPOINT: stop until I answer.
- ANALYZE (prompt 04): build vs adopt for each change. Covers safe-Rust sandbox, keychain, OAuth/PKCE and process-group crates (G-RT13). No unsafe, and wasm must keep building.
- PLAN (prompt 05):
  - Write one OpenSpec change per PLAN.md change, with acceptance scenarios that include every SCN-RT-* scenario from the spec. Gated-out items become explicit cuts, not silent drops.
  - Define waves and file grants.
  - Run adversarial review of the plan through the cross-model gateway: judge must not be the producer, at most 2 rounds. Fix what it finds.
  - CHECKPOINT: show me the plan and the review verdict; stop until I accept.
- EXECUTE (prompt 06):
  - One wave at a time. Each parallel agent uses its own CARGO_TARGET_DIR. TDD; tests in *_tests.rs. An integrator task closes each wave, and cargo xtask ci must be green.
  - security-privacy-reviewer reviews every Track B change that touches credentials, sign-in, process spawning or consent.
  - CHECKPOINT after each wave: report what landed, the evidence and anything cut.
- BOUNDARY AND REFLECT (prompt 07):
  - Run 5.9 and the B9 definition of done: screenshots in both themes compared against the prototypes, contrast matrix, scenario evidence and independent reviews.
  - Then my acceptance, then /kbd-reflect.

## Rules that are easy to get wrong here
- Never commit the private planning folders (plans, OpenSpec changes, KBD state); quote nothing of them in public docs beyond a change or task id. docs/design/ is public in this fork.
- Commit only when I say so. Run cargo xtask ci before every commit. Commit messages end with "Human-reviewed, AI-assisted, nobody's mailbox harmed." and carry no AI Co-Authored-By trailer.
- Colours come only from Tokens; text goes through tl!; every ui.* action is a CommandSpec in UI_COMMANDS; unsafe_code stays forbidden.
- **Amended (G-RT2 b, G-RT6 b, G-RT12 owner-specified, no per-token billing):** Claude Code runs unmodified on the owner's own login (a subscription included) with the real HOME; MailCraft offers no claude.ai login. Codex runs unmodified on the owner's own ChatGPT login in the owner's CODEX_HOME, with a per-run HOME and --ignore-user-config; MailCraft never runs codex login and holds no Codex key. No runtime needs an API key for Claude or Codex. Never read another CLI's credentials. Every runtime runs tool-free under the containment in spec §7, and any tool event kills the run.
- Keep the agent approval modal, bound, defaulting to No, with its 50 s timeout.
- If a fact in docs/design turns out wrong in code or vendor docs, record it in decision-log.md and tell me. Don't build on it.

Start with the baseline step now.
```

**Why one phase:** you asked to start from the code as it is. The split into two phases in `PLAN.md` assumed the earlier AI phase would close first. KBD now shows that phase as cancelled, so this kickoff turns its entry criteria into a baseline check instead.
