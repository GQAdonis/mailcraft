# KBD prompts for the MailCraft design work

Ready-to-paste prompts for a **Claude Code lead agent running Opus 5.5** with the KBD method installed, started in the root of this repository.

Each prompt is self-contained: it names the files to read, the stage to run, what to produce, and where to stop. Run them **in order**, one stage per session if context gets large. Each prompt ends at a hand-off point where the owner is asked.

**Fastest start:** `KICKOFF.md` is one paste that creates a single phase covering Part A and Part B from the current code and runs the whole loop, using prompts 02–07 for each stage. It has been run: the phase `mailcraft-ui-refresh-and-agent-runtimes` is the one these prompts produced, and the record below is what its gates decided. A later phase starts from that record, not from the gates as first proposed.

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
- The private planning folders (the gitignored plans, OpenSpec changes and KBD state) are never committed, nor quoted in public docs beyond a change or task id. `docs/design/` is public in this fork.
- Commits end with `Human-reviewed, AI-assisted, nobody's mailbox harmed.` and carry **no** AI `Co-Authored-By` trailer.
- Run `cargo xtask ci` before every commit. Each parallel agent uses its own `CARGO_TARGET_DIR`.
- Adversarial reviews go through the cross-model gateway (judge ≠ producer), with at most 2 rounds per artifact.

## The record of the phase these prompts ran

Phase `mailcraft-ui-refresh-and-agent-runtimes`, one phase with both tracks (KICKOFF.md). `AI-SETTINGS-SPEC.md` carries the amendments below in place.

### Gate answers

| Gate | Answer |
|---|---|
| G1–G18 (Part A) | Recommendations accepted. G6 c: keep the accepted-query pills. G9 a: ellipsis. G14: `sync` demoted as a special case. G16 a: focus rings on touched controls only. G17 a: query chips cut. G18 a: Enter opens the selected message |
| G-RT0 | a: Track A, then Track B |
| G-RT1 | a: on-device first after a one-time yes |
| **G-RT2** | **b (owner's words):** the owner signs in to the **unmodified** Claude Code CLI with their own Claude subscription; MailCraft runs it contained and tool-free and never reads, stores, passes or intermediates the login, and offers no claude.ai login of its own |
| **G-RT3** | **a:** Sign in with ChatGPT now, with OpenAI's dynamic per-account registration (`dynamic_agent_client`, a persisted `ext_agent_host_id`); no build-time client id, no owner registration |
| G-RT4 | a: Codex included, off by default |
| **G-RT5** | **b: OpenCode deferred, and cut from this phase** (PLAN.md row B5.8) |
| **G-RT6** | **b:** Codex uses the owner's own `CODEX_HOME`, where the owner's login lives |
| **G-RT12** | **Owner-specified:** Codex runs on the owner's own ChatGPT plan login, made by the owner with `codex login`; MailCraft never runs `codex login`, never reads `auth.json` and holds no Codex key |
| G-RT7 | a: detect and use the installed `~/.uar/bin/universal-agent-runtime`; nothing bundled |
| G-RT8, G-RT9, G-RT13 | a (G-RT13: safe crates; `sandbox-exec`'s deprecation recorded as a risk) |
| G-RT10 | `next` (fallback policy default) |
| G-RT11 | a: the direct Anthropic API deferred |
| Owner, standing | **No per-token billing, ever:** no runtime needs an API key for Claude or Codex; both use the owner's plan and its quota |
| G-BASE, G-METHOD, G-PRIOR | a, a, a: repair the baseline to green first; per-wave build and tests with `cargo xtask ci` at each wave's integrator task; the earlier phase's live scenarios become regression scenarios |
| G-RUNS | Pre-plan runs on the owner's existing logins with synthetic prompts: Claude Code needs the real `HOME` (containment option i); Codex needs `--ignore-user-config` |
| Owner, W3 checkpoint | The chip says "AI: your typed searches go to ‹host›" when a provider that leaves is active and no account consented; the ribbon's AI group stays icons-only |

Entry checks recorded during W7 (they decide what PRIVACY.md and the threat model say): Claude Code 2.1.326 gets `--safe-mode`, and the owner's `CLAUDE.md`, skills and agents didn't reach the prompt; its `init` event comes only after the first input, so the prompt is written at once (provisional, for the owner to confirm). Codex 0.169.0 has no flag that disables skills and agent roles in `CODEX_HOME`: a stated residual. `universal-agent-runtime` 1.4.0-rc.5 can't be started as a managed instance safely, so UAR is external-only for it.

### Final change ids (21)

- **W0:** `baseline-repair`.
- **Track A:** `ui-theme-additions`, `ui-reading-measure`, `ui-list-and-chrome-polish`, `ai-status-chip`, `ai-narrow-badge`, `ai-discoverability`, `ai-answers-sources-and-provenance`, `agent-console`.
- **Track B:** `ai-runtime-model`, `ai-runtime-commands`, `ai-runtime-detection`, `ai-runtime-containment`, `ai-runtime-claude-code`, `ai-runtime-codex`, `ai-runtime-chatgpt-siwc`, `ai-runtime-uar`, `ai-runtime-chain`, `ai-runtime-settings-ui`, `ai-runtime-docs`.
- **W10:** `phase-boundary`.
- **Cut:** `ai-runtime-opencode` (B5.8, G-RT5 b); the direct Anthropic API (G-RT11); a Windows sandbox, so Codex isn't offered on Windows; and the PLAN.md §4 and §B4 cuts.

### Frozen scenario ids (43)

- Baseline: SCN-BASE-001, SCN-BASE-002, SCN-BASE-003.
- Part A: SCN-MEASURE-001, SCN-CHIP-001 (amended for the W3 chip answer), SCN-AGENT-001, SCN-AGENT-002, SCN-AGENT-003, SCN-ENTER-001, SCN-A11Y-001, SCN-CONTRAST-001.
- Part B: SCN-RT-DETECT-001, SCN-RT-DETECT-002, SCN-RT-PATH-001, SCN-RT-AUTH-001 (amended), SCN-RT-AUTH-002 (amended), SCN-RT-SIWC-001, SCN-RT-SIWC-002, SCN-RT-CONTAIN-001 (OpenCode removed), SCN-RT-CONTAIN-002, SCN-RT-ARGV-001, SCN-RT-SANDBOX-001, SCN-RT-WHERE-001, SCN-RT-CHAIN-001, SCN-RT-CHAIN-002, SCN-RT-CHIP-001, SCN-RT-BADGE-001, SCN-RT-SETTINGS-001, SCN-RT-MIGRATE-001, SCN-RT-UAR-001, SCN-RT-PRIV-001, SCN-RT-LIVE-001.
- Regression (G-PRIOR a): SCN-PRIOR-RETRIEVE-001, SCN-PRIOR-SEARCH-001, SCN-PRIOR-ANSWER-001, SCN-PRIOR-INJECT-001, SCN-PRIOR-PRIVACY-001, SCN-PRIOR-LOOPBACK-001, SCN-PRIOR-OPTIN-001, SCN-PRIOR-STREAM-001, SCN-PRIOR-FAIL-001, SCN-PRIOR-SAFETY-001, SCN-PRIOR-MCP-001.
