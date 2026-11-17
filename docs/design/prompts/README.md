# KBD prompts for the MailCraft design work

Ready-to-paste prompts for a **Claude Code lead agent running Opus 5.5** with the KBD method installed, started in the root of this repository.

Each prompt is self-contained: it names the files to read, the stage to run, what to produce, and where to stop. Run them **in order**, one stage per session if context gets large. Each prompt ends at a hand-off point where the owner is asked.

**Fastest start:** `KICKOFF.md` is one paste that creates a single phase covering Part A and Part B from the current code and runs the whole loop, using prompts 02–07 for each stage.

| # | File | Stage | Phase |
|---|---|---|---|
| 0 | `00-preflight.md` | Check entry criteria; choose phase order (G-RT0) | both |
| 1 | `01-create-phase.md` | Create the phase with `/kbd-new-phase` and write `goals.md` | A or B |
| 2 | `02-assess.md` | `/kbd-assess`, verification list, decision gates batch | A or B |
| 3 | `03-owner-decisions.md` | Present the gates as one question form; record answers | A or B |
| 4 | `04-analyze.md` | `/kbd-analyze`: build vs adopt; library candidates | A or B |
| 5 | `05-plan.md` | `/kbd-plan`: OpenSpec changes, waves, grants, routing, scenarios; adversarial review | A or B |
| 6 | `06-execute-wave.md` | `/kbd-execute`, one wave at a time, with integrator task | A or B |
| 7 | `07-boundary-and-reflect.md` | Boundary run, reviews, owner acceptance, `/kbd-reflect`, commit | A or B |

Replace `<PHASE>` with `window-refresh-and-agent-console` (Part A) or `agent-runtimes` (Part B). Replace `<PART>` with `A` or `B`.

**Rules every prompt repeats, because they are easy to get wrong here:**
- AGENTS.md is the authority. If anything in `docs/design/` conflicts with it, AGENTS.md wins; raise the conflict with the owner.
- `plan/`, `openspec/` and `.kbd-orchestrator/` are never committed. `docs/design/` is public in this fork.
- Commits end with `Human-reviewed, AI-assisted, nobody's mailbox harmed.` and carry **no** AI `Co-Authored-By` trailer.
- Run `cargo xtask ci` before every commit. Each parallel agent uses its own `CARGO_TARGET_DIR`.
- Adversarial reviews go through the cross-model gateway (judge ≠ producer), with at most 2 rounds per artifact.
