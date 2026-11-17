# Implementation plan — window refresh, AI/agent surfaces and agent runtimes

**For:** a Claude Code lead agent (Opus 5.5) running the KBD method in this repository.
**Inputs:** `docs/design/DESIGN-PROPOSAL.md` and the clickable prototype `docs/design/index.html` (Part A); `docs/design/AI-SETTINGS-SPEC.md`, `docs/design/settings.html` and `docs/design/research/AGENT-RUNTIMES.md` (Part B). Prototypes are scripted; open them in a browser.
**Prompts:** ready-to-paste KBD prompts for the lead are in `docs/design/prompts/`.
**Status of this document:** a proposal for two new KBD phases (Part A: `window-refresh-and-agent-console`; Part B: `agent-runtimes`). Nothing in it is decided until the owner answers the decision gates in §3. It was reviewed adversarially; the review result and what changed because of it are in §11.
**Written:** 2026-11-17, against the working tree of `feat/agentic-ai-integration` (uncommitted AI work present).

> **Publication.** The owner decided (2026-11-17) that `docs/design/` is **public in this fork**: the proposal, the prototype, this plan and the review records are committed on purpose. This is a deliberate exception to the AGENTS.md rule that keeps specs and plans private. That rule still applies to everything the phase itself produces: `plan/`, `openspec/` and `.kbd-orchestrator/` stay uncommitted. Paths this plan cites under those folders (for example `plan/design/ai-*.md` and `.kbd-orchestrator/phases/...`) exist only in the maintainers' working copies. The prototype loads Inter from the repo's own `assets/fonts/`, which is already listed in `ATTRIBUTION.md`, so this folder adds no new asset files.

---

## 0. TL;DR for the lead

1. **Do not start until `agentic-ai-integration` and its child `agentic-ai-scenario-repair` have closed and their code is committed.** The AI code this phase builds on (`crates/ai`, `crates/ui-egui/src/ai/*`, `crates/engine/src/cmd/ai/*`) is untracked and mid-wave right now; editing the same files (`chrome.rs`, `lib.rs`, `ai/*.rs`, `backstage.rs`) in parallel guarantees conflicts and would invalidate that phase's boundary evidence.
2. **The proposal is a design target, not a spec.** Several of its choices contradict decisions the owner (or the lead, with owner-overridable defaults) already recorded for the AI phase. §3 lists them as decision gates with a recommendation each. Settle them at **assess**, before any change is written.
3. **Proposed phase:** `window-refresh-and-agent-console`, a new top-level phase after the AI phase reflects. Nine OpenSpec changes in four waves (§6). Two tracks: *window polish* (no AI dependency) and *AI/agent surfaces* (depends on committed AI code).
4. **Part B adds a second phase, `agent-runtimes`** (§B0–B11 at the end): Apple on-device first on Mac, OpenAI-compatible providers, ChatGPT plan via Sign in with ChatGPT, Claude Code, Codex, OpenCode and the Universal Agent Runtime, with detection, sign-in automation and token management. **Vendor policy shapes it:** Claude subscriptions can't be used by other apps, and ChatGPT plans only through OpenAI's own Sign in with ChatGPT (research §0). Run Part A first, then Part B (§B1).
5. **The hardest parts are not visual.** They are: keeping the agent-approval safety model while adding a non-modal Agents panel (§5.8), passing the full `theme_contrast_tests.rs` matrix with every new token (§5.1), and capping the reading measure without breaking HTML newsletters that need the full width (§5.2).

---

## 1. What exists today (verified in code, 2026-11-17)

Line numbers drift; re-read before editing.

| Proposal item | Current code | Gap |
|---|---|---|
| Reading measure ~72ch | `reading/view.rs:457` computes `width = floor(min(available, MAX_TEXT_WIDTH)/4)*4`; `MAX_TEXT_WIDTH = 4000` (`reading.rs:92`) bounds layout work only; `MAX_BODY_WIDTH = 860` (`reading.rs:89`) applies to pictures only. Body `BASE_SIZE 13`, `LINE 1.45` (`reading/layout.rs:42-44`). | **No readable-measure cap.** Width also feeds `@media` evaluation (`crates/html/src/media.rs`). |
| Sender "Name&lt;addr&gt;" | `reading.rs:509-563`: name `semibold(13)`, address `regular(11.5)` in `text_dim`, gap `ADDRESS_GAP = 4.0`. | Already distinct weight/size/colour. The screenshot defect is a 4 pt gap plus a likely semibold-face fallback (`theme.rs:1167` chain). Small fix. |
| Diagnostics pills in title bar | `chrome.rs:31-74` `diagnostics_pill`, `report_pill`; placed at `chrome.rs:384-387`. Doc comment `chrome.rs:29`: placed there **on purpose so testers find it (decision D1)**. Help tab already has both (`ribbon.rs:1089-1090`). | Removal is trivial; the decision is not (gate G6). |
| Flagged-row tint | `mail_view.rs:826-832` `row_fill`, painted `:919-946` (selection on top of a flagged row at `gamma_multiply(0.72)` + 1 px edge). Tokens `flagged_row` light `#FCF0F2`, dark `#3A2A2E`. Distinctness unit test `mail_view.rs:1788-1817` and `theme.rs:1646-1672`; contrast requires text/text_dim/overdue on `flagged_row` ≥ 4.5 (7 in HC). `auto_replies.rs:930` also uses it. | Lighter tint must still pass "distinct from surface/selection/hover". |
| Ellipsis in list | Sender/subject already ellipsized via `theme::paint_clipped` (`theme.rs:1309-1319`). **Preview** (`mail_view.rs:1182-1194`) clips at a word with **no mark, on purpose**. | Decision gate G9 (cheap either way). |
| Type scale | `theme.rs:1341-1345`: Body 12.5, Small 10.5, Heading 17, Mono 12. `TypeRung` (`theme.rs:1232-1276`) used ~11×. **387 literal font sizes** in non-test UI code (calendar_view 56, reading 22, people_view 22, backstage 19, tasks_view 18…). | A global scale change is a large, risky refactor (gate G4). |
| Spacing/radius | No spacing or radius tokens; ~185 literal radii, ~449 `vec2(` literals, ~210 `add_space(` literals. | Tokenize only what this phase touches. |
| Ribbon disabled big button | `ribbon.rs:467` `Item::big("sync", …,"ui.sync")`; enabled via `lib.rs:2249-2256`. Simplified (default) and Classic modes; fitting in `ribbon/fit.rs`. | Needs a "demote when disabled" rule in both modes. |
| Module bar labels | `nav.rs:63-98`, `RAIL_W 48`, tooltip only. `module_foot()` (`nav.rs:137`) already shows names at the foot of the folder pane when they fit. | Wider labelled rail is new; `module_foot` may already satisfy the need (gate G8). |
| Status bar item | `chrome.rs:444`, right-to-left block from `:508`; pattern: a `status_item(app, ui, &t)` in its own module (`updates.rs`, `auto_replies.rs`). | Adding the AI chip is a well-trodden path. |
| Search + AI entry | `chrome.rs:91` `search_box`; right end has scope menu, Advanced, and the AI **Find by Description** button (`ai::narrow::button`, `chrome.rs:161/269`). Box text runs `view.search` live on every change. | The proposal's in-box Search|Ask switch conflicts with a recorded rule (gate G2). |
| Narrowing | `ai/narrow.rs`: Suggest Search → editable proposal with pills → Use This Search / Try Again / Answer Instead / Write Your Own / Stop / Options. | Mostly naming + a local/remote badge. |
| Answers | `ai/answers.rs` right-hand `Panel::right` (360 pt, 280–560) placed at `lib.rs:5389`; streaming, Stop, ≤3 citation chips, links shown as text. Opened via `ui.answers` or Answer Instead; **no ribbon button**. "Answered by" + fallback notice = child task 6.6 (pending). | Proposal puts answers *in* the reading pane (gate G1). |
| AI settings | File › Options › **"AI assistance"** (`backstage.rs:788-789` → `ai/settings_page.rs`, being written in child wave 3): None/local/gateway/remote, primary + backup (Apple on-device, confirmed once), Test Connection, context window, per-account consent via `Dialog::Confirm`. | Proposal's simpler Off/On-this-computer model omits gateway, backup, confirm-on-device (gate G5). |
| Agent approval | `agent_gate.rs:447-505`: one `egui::Modal` "An Agent Is Asking", queue ≤ 8 (`:29`), 50 s timeout = no (`:26`), No is default, Enter/Esc = no (`:432`), yes is **bound** to what was shown (`agent::bind::Shown`), control-channel input suppressed while open (`app.synthetic.clear()`), exposed in `ui.inspect` as `agentQuestion` (`control.rs:157`). | **No activity log, no connected-agents list, no disconnect, no client identity** on requests. |
| Visual verification | `ui.screenshot` over the control channel (`control.rs:381`); headless `crates/ui-egui/examples/ui_shot.rs` (JSONL script, 1440×900 @2×, fixed clock, wgpu); `egui_kittest 0.37` harness tests in `tests.rs` (`frames()` `:51`, `window()` `:492`, hit-map clicks). No golden-image comparisons; PNGs saved when `MAILCRAFT_SHOTS_DIR` is set. | Use these; don't add a new screenshot stack. |

Versions: egui / eframe / egui_extras / egui_kittest **0.37.0** (workspace `Cargo.toml:147-152`).

**Command ids this plan relies on — all verified to exist (2026-11-17):** `ui.findByDescription` (`lib.rs:1238`), `ui.answers` (`lib.rs:1244`), `ui.aiSettings` (`lib.rs:1250`), all dispatched at `lib.rs:2760` → `ai/mod.rs`; `ui.openWindow` (`lib.rs:1488`); engine `mail.select` (used across `crates/engine/src/cmd/*_tests.rs`); `ui.copyDiagnostics`, `ui.reportProblem` (Help tab). No change in this plan creates any of them; new ids it creates are `ui.agents` and `ui.agentAccess` (5.8) and, only if the owner wants it, a line-length setting (5.2).

**`--allow` (from `docs/mcp.md`):** the owner starts the MCP bridge with `--allow send|delete|accounts|mailbox|settings|input` to let an agent do that class *without being asked*. Headless it is the only way an agent can do owner-class things; bridged it skips the question for that class. `input` is never asked about. The activity log (5.8b) must distinguish "allowed in advance by `--allow`" from "owner said yes".

---

## 2. Rules that bind every change (from AGENTS.md and `.kbd-orchestrator/constraints.md`)

- **Never crash:** no `unwrap/expect/panic!/unsafe` outside tests; `hostile_params_never_panic` stays green for any new command.
- **Tests in their own files** (`*_tests.rs`, `tests.rs`, `tests/`). `chrome.rs`, `theme.rs`, `mail_view.rs` are on `xtask/inline-tests.txt` — the list may only shrink; do not add inline tests there.
- **Colours only from `theme::Tokens`**, in *every* palette: 4 accents × light/dark (+ Blue dark style) × high contrast on/off. `theme_contrast_tests.rs` must pass (4.5 text / 3.0 graphic; 7.0 in HC). `blue_normal_is_the_look_that_shipped` pins the default Blue tokens — **changing `flagged_row` or any pinned token means updating that test deliberately, with the owner's yes (gate G3).**
- **Text through `tl!`/`tln!`/`tlc!`**, literal arguments only; run `cargo xtask i18n`; the `.pot` must be current.
- **Everything is a command:** new window actions are `ui.*` entries in `UI_COMMANDS` (`lib.rs:1179`) dispatched in `MailApp::ui_command` (`lib.rs:2596`), with `enabled` logic in `MailApp::enabled` (`lib.rs:2240`). Agent classification of any new command is decided by **engine-engineer** (sole owner of `agent.rs`); window-only AI/agent-management commands are owner-only.
- **Layering:** egui only at L6; engine does no server I/O; **wasm keeps building** (the `ai` UI module compiles for wasm and hides itself at runtime when no provider service exists — follow that pattern).
- **Icons drawn in `icons.rs`;** no vendor iconography; any asset file gets an `ATTRIBUTION.md` row.
- **Visual check:** look at the result in light, dark, high contrast and 200 % interface size (DESIGN.md + constraint `ui-checked-visually`).
- **Docs as fragments:** `docs/log.d/…` and `packaging/release-notes.d/…` per change.
- **Commits:** end with `Human-reviewed, AI-assisted, nobody's mailbox harmed.` and **no** AI `Co-Authored-By` trailer (AGENTS.md overrides harness attribution).
- **`cargo xtask ci`** before every commit. Each parallel agent uses its own `CARGO_TARGET_DIR`.

---

## 3. Decision gates — settle at **assess**, before writing changes

Each gate is a real conflict between the proposal and what is built or recorded. Present them to the owner in one batch (question form, recommended answer pre-selected). Record answers in the new phase's `decision-log.md`.

| Gate | Conflict | Options | Recommendation (why) |
|---|---|---|---|
| **G1 Answer placement** | Proposal: answer replaces the reading pane, "Back to the answer". Built + recorded (AI phase task 3.1 default): right-hand pane so a cited message opens in the reading pane while the answer stays visible. | (a) keep right pane; (b) reading-pane takeover; (c) both, owner toggles | **(a)** Keeps evidence and answer side by side, which is the point of citations; no rework of a just-built surface. Port the proposal's *content* improvements into the pane (sources list, provenance line, Stop/partial wording). |
| **G2 Ask entry point** | Proposal: one box with a Search \| Ask switch and sentence auto-detection in Search mode; Ctrl/⌘K. Recorded rule (narrowing brief §3): **the search box's own text is never sent to a model**, typing keeps its live filter, no default shortcut. | (a) keep separate Find by Description button; add a ribbon **AI** group with *Ask Mail* (`ui.answers`) and *Find by Description*; (b) in-box mode switch where Ask mode is explicit and never auto-detected; (c) proposal as drawn | **(a)** It preserves the privacy rule and the live filter with zero ambiguity, and fixes the real discoverability gap (answers have no ribbon entry today). If the owner wants (b), the switch must default to Search on every launch, Ask mode must be visually unmistakable, and nothing typed in Search mode is ever sent. **Never (c)**: auto-detecting "a sentence" silently changes where typed text goes. |
| **G3 Pinned look** | Lighter `flagged_row`, possible type/spacing changes alter tokens pinned by `blue_normal_is_the_look_that_shipped`. | (a) allow re-pinning with recorded screenshots; (b) keep pinned values | **(a)** for `flagged_row` only; everything else additive. |
| **G4 Type scale** | Proposal: 11/12/13/15/16/20, 13 pt body. DESIGN.md: "New surfaces add no new sizes"; 12.5 body, 10.5 small; 387 literal sizes. | (a) floor on touched surfaces only: the global `Small` style stays 10.5 for modules this phase does not touch; add a new 11 pt rung and use it on every surface this phase touches; (b) full scale migration; (c) none | **(a)** Fixes the readability issue (tiny dim text) with a bounded diff. A full migration is its own phase (calendar alone has 56 literals) and would need screenshot review of every module. |
| **G5 Settings model** | Proposal: Off / On this computer + per-account Allow/Revoke with "I understand" tick. Built: "AI assistance" with None/local/gateway/remote, primary+backup, Test Connection, confirm-on-device, consent dialog defaulting to No, "Stop" not "Revoke", provider-level remote question. | (a) keep the built settings model unchanged; adopt only the status chip (5.4), which reads settings state and never edits `ai/settings_page.rs`; (b) redesign to proposal | **(a)** The built model encodes owner decisions (gateway kind, backup, apfel confirm) the proposal didn't know about. Do not add the tick box: the decided consent is a dialog whose default focus is No, which already forces an explicit act. |
| **G6 Diagnostics pills** | Proposal moves them to Help. Code comment: placed in title bar on purpose for testers (D1). | (a) keep for tester builds, move to Help for release builds; (b) remove now; (c) keep | **(a)** if a build/profile flag already distinguishes tester builds; otherwise **(c)** until the tester programme ends. Ask the owner. |
| **G7 Agents panel** | Proposal: non-modal drawer with pending approvals, connected agents, activity log; replaces the modal. Built: modal with bound yes, 50 s timeout, input suppression. | (a) keep the modal for the **question**, add a non-modal **Agents** panel for history and connections; (b) move questions into the drawer | **(a)** The modal is a safety property (nothing else can act while the owner decides; the yes is bound to what was shown). A drawer question would need to re-prove all of that. The panel adds visibility without touching the decision path. |
| **G8 Module labels** | Proposal: widenable labelled rail. Existing: `module_foot` shows names when they fit. | (a) rely on `module_foot`, add label on hover delay only; (b) widenable rail | **(a)** unless the owner wants the rail; cheaper, already tested (`module_foot_tests.rs`). |
| **G9 Preview ellipsis** | Preview is clipped with no mark on purpose. | (a) add `…`; (b) keep | Owner's call; (a) is a one-line `overflow_character` change but alters the shipped look. |
| **G11 Approval cards** | Proposal 2.7 shows durable per-request cards in a drawer (Send it / Delete it / Decline). Built: the modal decides within 50 s and blocks the window, so a "Waiting" list in a side panel could never be acted on. | (a) keep the modal; the panel shows no waiting cards; (b) durable queued approvals (remove/lengthen the timeout; agents wait) | **(a)** Lengthening the wait changes the agent contract (the channel gives up at 60 s) and the safety argument. Revisit only as its own change with a security review. |
| **G12 Activity log content** | Proposal: "what agents searched, read and asked for". Plan: ids and counts only, no subjects/addresses/query strings, memory only. | (a) ids + counts, memory only; (b) include subjects/queries; (c) persist to disk | **(a)** The log is a convenience view; storing what agents searched duplicates mail content in a new place and widens the privacy surface. (b)/(c) need a PRIVACY.md change. |
| **G13 Line-length setting** | 5.2 could add File › Options › Mail › "Limit line length". | (a) no setting, always on; (b) setting, default on | **(a)** Fewer moving parts; add the setting later if anyone asks. If (b), the integrator adds the `ui.*` command and the `backstage.rs` row. |
| **G14 Ribbon demotion rule** | 5.3 can demote only `ui.sync`, or any disabled Big item with no menu. | (a) special-case `sync`; (b) general rule | **(a)** Smallest change; a general rule reshuffles other tabs. |
| **G15 Kill-switch persistence** | 5.8's "Stop agent access". | (a) session only; (b) remembered in `ui.json` | **(a)** A remembered off-switch silently breaks the owner's agents after restart; session-only is easier to reason about. |
| **G10 Drafts** | Proposal includes "Suggest a draft". AI compose is cut from the AI phase and is an open product decision (`PRODUCT.md`). | (a) out of scope; (b) include | **(a)** — needs its own phase with its own safety review (model output entering a message that will be sent). |

Gates G1, G2, G5, G7 change scope; the others change details. If the owner is unavailable, proceed with the recommendations and mark them "lead default, owner may overturn", as the AI phase did.

---

## 4. Phase definition

**Name:** `window-refresh-and-agent-console` (top-level; created with `/kbd-new-phase` after the AI phase reflects — or `/kbd-next-phase` if its reflection recommends it).

**Entry criteria (all required):**
- `agentic-ai-integration::agentic-ai-scenario-repair` passed B4c and the owner accepted it; the parent reflected.
- AI code committed on a branch the new phase starts from (record the commit hash in `prior-context.md`).
- Specifically confirmed done in the child phase's `tasks.md` (5.6 and 5.4 build on them): **6.4, 6.6** (streamed answer extraction; "Answered by" + fallback notice), **7.2–7.6** (AI settings page, `answerText` in `ui.inspect`, host side, context window). If the child closed with any of these cut, re-plan before starting: 5.4 and 5.5 consume 7.2 (the settings surface that `ui.aiSettings` opens); 5.6 consumes 6.4 and 6.6; 5.9's answer-pane checks consume 7.3.
- `cargo xtask ci` green on that commit.

**Goals (proposed `goals.md`):**
- Follow the KBD process in order: assess → analyze → plan → execute → reflect.
- Make the reading pane and message list easier to read without changing the Outlook arrangement: readable measure, clearer sender header, quieter flag treatment, no text below 11 pt on touched surfaces.
- Make AI help discoverable and its privacy state always visible: ribbon AI group, status-bar AI chip, sources and provenance in the answer pane, local/remote badge in narrowing.
- Give the owner visibility over external agents: an Agents panel listing connected agents, a pending count, and an activity log — while keeping the modal, bound, default-No approval unchanged.
- Hold every AGENTS.md rule; every new token passes the full contrast matrix; wasm keeps building.

**Out of scope (explicit cuts):** AI-drafted replies (G10); full type-scale/spacing migration (G4); in-box auto-detected Ask (G2c); redesign of AI settings (G5b); durable approval cards (G11b); Calendar/People/Tasks restyling; Android/iOS.

### 4.1 Proposal coverage — every item has a home or an explicit cut

| Proposal item (DESIGN-PROPOSAL.md) | Where | Note |
|---|---|---|
| 1 Reading measure | 5.2 | |
| 1 "Header and attachments share the same column" | 5.2 | header block and attachment strip use the same measure as the body |
| 2 Sender header gap + dim address | 5.2 | gap 4→8; address already dim |
| 2 Address at 12 px | **cut** | stays 11.5 (≥ the 11 pt floor) |
| 2 "To" on its own line, ellipsized | **assess check** | the header already draws To separately (`reading.rs` header); confirm it ellipsizes, else add in 5.2 |
| 3 Diagnostics to Help | 5.3 | G6 |
| 4 Quieter flag | 5.1 (token value) + 5.3 (rendering) | G3 |
| 4 3 px selection edge | **cut** | keep the current 1 px edge + fill; a thicker edge is a look change outside this phase |
| 5 Preview ellipsis | 5.3 | G9 |
| 6 Type ≥ 11 pt | 5.1 (rung) + 5.3 (apply) | G4a; touched surfaces only |
| 7 Radii/4-8 grid | **cut** except consts this phase uses (5.1) | G4 |
| 8 Disabled ribbon button small | 5.3 | |
| 9 Module labels | 5.3 | G8 |
| Focus ring on every control | **assess check** | `Tokens.focus_ring` exists; assess lists controls that lack it; fix only on touched surfaces |
| Honest empty states | **assess check** | verify list/search empty states exist and read honestly; fix in 5.3 if missing |
| ↑/↓/Enter in the list | **assess check** | expected to exist (`keys.rs`); confirm, else cut |
| Responsive ≥1280 / 1024 / 768 | 5.9 verifies **1024, 1280, 1440** | desktop window widths; folder pane collapse already exists |
| Responsive < 768 (two screens + Back) | **cut** | browser-on-phone layout; mobile users are served by the native apps |
| 2.1 Search \| Ask in one box | 5.5 | G2 (recommend separate entry points) |
| 2.2 Narrowing popover (proposal, pills, accept) | built; 5.7 adds the badge | names per G2 |
| 2.2 Accepted query as pills + Clear above the list | **assess check** | the search box already renders query pills (`search_pills.rs`) and Escape clears; if that satisfies the owner, cut; else a small addition to 5.7 |
| 2.2 "AI off → search as typed + message" | **cut** | conflicts with the recorded rule that Find by Description explains and links to Options when no model is set up |
| 2.3 Answers | 5.6 | G1 (right pane) |
| 2.4 Drafts | **cut** | G10 |
| 2.5 Settings + consent | **cut** (keep built) | G5 |
| 2.6 Status chip | 5.4 | |
| 2.7 Agents drawer | 5.8 | G7, G11, G12; no per-agent Disconnect |
| Part 3 `cite` token | **cut** | reuse `accent_soft` (that is what the proposal's value is) |
| Part 3 `suggest_*` tokens | **cut** | drafts cut |
| Part 3 `danger_soft` | **cut** | only used by approval cards (G11a) |
| Part 3 `ai_local*`, `ai_remote*`, `flagged_row` | 5.1 | |

---

## 5. Changes (OpenSpec), with implementation notes

Each change: `openspec/changes/<id>/{proposal.md, design.md, tasks.md, specs/<capability>/spec.md}`, validated with `openspec validate --strict`. Suggested capability names in brackets.

### 5.0 Assess/analyze deliverables (not an OpenSpec change) — designer, lead
These are stage artifacts in `.kbd-orchestrator/phases/<phase>/` (`assessment.md`, `decision-log.md`, `evidence/`), produced **during assess/analyze**, before any OpenSpec change exists.
- Record gate answers; update DESIGN.md (git-excluded) with the decisions and the new tokens' intent.
- Run the three "assess check" rows of §4.1 (focus ring, empty states, list keys) and record the result.
- Capture **before** screenshots with `examples/ui_shot.rs` (Mail list + reading pane, flagged+selected row, answer pane streaming and done, narrowing proposal, Options › AI assistance, agent question) in light/dark/HC and at 200 %. Store under the phase `evidence/`, not in git.
- Write a short brief per surface touched (Impeccable `shape`), as the AI phase did, in `plan/design/`.

### 5.1 `ui-theme-additions` [ui-theme] — ui-engineer
- Add tokens, each in every palette and in `high_contrast()` (`theme/accent.rs:163`): `ai_local`, `ai_local_soft`, `ai_remote`, `ai_remote_soft` (chip + badge), and adjust `flagged_row` (if G3a).
- Add every new pair to `theme_contrast_tests.rs` `PAIRS` (chip text on its soft fill ≥ 4.5; 7.0 in HC). Do not encode meaning in colour alone: the chip also carries an icon and words.
- Add a **new** `TypeRung` at 11 pt (e.g. `TypeRung::Note`) and use it only on touched surfaces. **Do not raise the global `Small` style** — that would change every module this phase cuts out of scope.
- **Ownership:** this change owns every token *value*, including `flagged_row`. Later changes only *use* tokens; if one needs a new value it goes back to this change's owner (§6 grant).
- Optional, bounded: `Space`/`Radius` consts in theme.rs for values this phase uses; do **not** sweep other modules.
- Tests: contrast matrix; flagged distinctness tests still pass; re-pin `blue_normal_is_the_look_that_shipped` only for G3-approved tokens.

### 5.2 `ui-reading-measure` [ui-reading] — ui-engineer
- In `reading/view.rs` (`show()` ~`:457` and `env_of()` ~`:371`), introduce a **measure width** used for flowing text blocks only. Definition (reproducible): `measure = min(width, 72 × glyph_width(body FontId at BASE_SIZE, '0'))` — the CSS `ch` convention. API verified in epaint 0.37: `Fonts::glyph_width(&FontId, char) -> f32` (`epaint/src/text/fonts.rs:355`), already used in `reading/layout.rs:1405`. Use the window's body font (not a message's own font or a fallback face) so the measure is stable per interface size; non-Latin text wraps within the same width. Unit test with a fixed font: measure equals 72 × the '0' advance.
- **Keep `@media` evaluation and tables/pictures on the full width.** Implementation choice for `design.md`: either (i) lay out with full width but constrain paragraph blocks to `measure` inside `reading/layout.rs` (block-level max width), or (ii) lay out at `measure` only for messages with no tables wider than `measure`. Prefer (i): it keeps one layout pass and newsletters intact. Left-align the column (do not centre) to match the current reading-pane edge.
- Plain-text messages: same measure. Quoted text: same measure.
- Line-length setting per G13 (default: none).
- 5.2 registers the **reading pane** region with the type-floor helper (below) and asserts the sender header and body meet the 11 pt floor.
- Sender header: raise `ADDRESS_GAP` to 8; verify the semibold face actually loads on Windows/macOS/Linux/browser (log which face resolved in a test using `SystemFont`); keep the existing wrap rule.
- Tests (`reading/header_tests.rs`, `reading/tests.rs`): a long paragraph wraps at ≤ measure; a wide table still gets full width; `@media` result unchanged for a fixture; text selection/copy unaffected (`reading/select.rs`).
- Risk: `reading/tests.rs` is 4,701 lines with layout assertions; expect fixture width updates. Budget for it.

### 5.3 `ui-list-and-chrome-polish` [ui-mail-list] — ui-engineer
- Flag treatment (G3): **rendering only** (the value comes from 5.1). Keep the flag glyph. Acceptance: a selected row's fill equals `selection` (or `selection_unfocused`) whether or not it is flagged — i.e. drop the `gamma_multiply(0.72)` blend in `mail_view.rs:919-946`. Test: unit-test `row_fill` for the four combinations (selected × flagged) and a harness test that a selected+flagged row still draws the flag glyph; then look at the screenshot.
- Preview ellipsis per G9 (`mail_view.rs:1182-1194`, `overflow_character: Some('…')`).
- Text below 11 pt on the list/status bar raised to the 11 rung (G4a).
- Diagnostics pills per G6 (`chrome.rs:384-387`); if moved, add both commands to the macOS menubar Help (`menubar.rs:240`).
- Ribbon: when `ui.sync` is disabled, render it as `Size::Small` in its group in **both** Simplified and Classic (`ribbon.rs:467`, `ribbon/fit.rs`), per G14 (default: special-case `sync`).
- Module labels per G8.
- Tests: `title_bar_tests.rs`, `nav_tests.rs`, `module_foot_tests.rs`, ribbon fit tests; harness test that the disabled sync item's rect height equals a small item's.
- **Type-floor test (applies to 5.2, 5.3, 5.4, 5.6, 5.7, 5.8).** 5.3 creates the helper once, in its own file `crates/ui-egui/src/type_floor_tests.rs` (granted to 5.3 in §6), exposing `assert_type_floor(harness, region_rect, 11.0)`. **Mechanism (verified in egui_kittest/epaint 0.37):** `Harness::output()` returns the last frame's `egui::FullOutput` (`egui_kittest/src/lib.rs:607`); walk `output.shapes`, take every `Shape::Text`, keep those whose rect intersects the region, and read each section's `galley.job.sections[i].format.font_id.size`. Sizes are in logical points, so the assertion is independent of `pixels_per_point` (it holds at 200 %). Region rects come from the existing hit map (`app.hit`). **Each change calls the helper from its own `*_tests.rs`** with its own region; nobody edits the helper after 5.3, so parallel agents never share a file. New surfaces use existing rungs only (DESIGN.md: "New surfaces add no new sizes"), with the 5.1 11 pt rung as the floor.

### 5.4 `ai-status-chip` [ai-status] — ui-engineer, ai-provider-engineer (read-only state)
- New module `crates/ui-egui/src/ai/status.rs` with `status_item(app, ui, &t)`, placed in the status bar's right-to-left block next to "This mailbox is on this device only".
- States, from the `AiProviders` trait (`lib.rs:327-391`) — **read-only, no new provider calls**: None → "AI off"; local only → "AI: on this computer"; any account with remote/gateway consent → "AI: remote allowed for {accounts}" (`tln!` for counts). Backup on-device counts as local.
- Click → `ui.aiSettings` (exists, §1). Accessible name equals the visible text. Register a hit (`app.hit.add`) for tests.
- **Non-colour signal is this change's job:** each state has its own icon, drawn here in `icons.rs` (AI off: a struck-through chip; on this computer: a chip; remote allowed: a cloud). Test: the three states differ in icon id and text, not only in colour.
- Browser build: the `ai` UI module is **not** cfg-gated today (`lib.rs:52` `mod ai;`, no `cfg` in `ai/mod.rs`); it compiles for wasm and is hidden at runtime when no provider service exists. Follow the same runtime rule — show the chip only when `Services.ai_providers` is present — rather than adding a new `cfg`.
- Tests: one per state; text truncates with `paint_clipped` at narrow widths; no provider I/O triggered by drawing (assert via a fake `AiProviders`).

### 5.5 `ai-discoverability` [ai-entry] — ui-engineer, engine-engineer (classification only)
- Ribbon **AI** group on the Mail Home tab (`home_mail()`, `ribbon.rs:408-469`): *Ask Mail* (`ui.answers`), *Find by Description* (`ui.findByDescription`), *AI Settings* (`ui.aiSettings`). Draw three new icons in `icons.rs` (spark, find-by-description, shield) — original, stroke-matched to the existing set.
- Ribbon group hidden when no AI provider service exists (the browser build today), using the same runtime check as the chip, not a `cfg`.
- **Privacy test for G2a (the recommended option):** typing in the search box issues only `view.search` (and suggestion lookups) — never any `ai.*` command — and Find by Description sends only what is typed into *its own* field in the popover (the narrowing brief's rule: "the search box's own text is never sent"). Assert both with a fake `AiProviders` that records every request.
- If G2b: implement the in-box Ask switch here instead; spec must state "text typed while Search is selected is never sent to a model" and include a test that asserts no `ai.*` command is issued from Search mode.
- Tests: command search finds the three; the group fits in Simplified mode at 1024 px without pushing Send/Receive behind `…` (check `plan()` output).

### 5.6 `ai-answers-sources-and-provenance` [ai-answers] — ui-engineer
- **Depends on child task 6.6** (Answered by + fallback notice) having landed; extend, don't duplicate.
- In `ai/answers.rs`: below the streamed text, a **Sources** list mirroring the citation chips (number, subject, sender, date), each opening the message in the reading pane (`mail.select`/`ui.openWindow` as the chips do). Keep the ≤ 3 citation limit; the list shows exactly the cited messages.
- Provenance line: which model, where it ran (local/gateway/remote host), how many passages were read, "message text was treated as data". Wording through `tl!`.
- Keep: no clickable links in answers, no conversation memory (the proposal's suggested follow-up questions are **out** unless the owner adds them — they would be static strings, never model output).
- Tests: sources list equals citation set; a citation to a message that no longer exists renders as "message no longer available" without panicking; partial answer after Stop keeps only sources cited so far.

### 5.7 `ai-narrow-badge` [ai-search-narrowing] — ui-engineer
- In `ai/narrow.rs` proposal stage: a badge "On this computer" / "Remote: {host}" / "Gateway: {host}" from the active route, using the 5.1 tokens. Host shown as the configured hostname only (no path, no key).
- Optional label alignment with the proposal (Use This Search stays the decided label unless the owner prefers "Run search").
- Tests: badge text per provider kind; badge never shows credentials.

### 5.8 `agent-console` [agent-activity] — agent-surface-engineer (bridge + control channel), engine-engineer (classes), ui-engineer (panel); reviewed only at 5.9
This is the riskiest change; keep its parts separable.

**(a) Client identity (control/MCP).** Verified 2026-11-17: `crates/mcp/src/server.rs:196` handles `initialize` but **does not read `clientInfo`**, and the control protocol (`docs/control-protocol.md:67-102`) carries only `"agent": {"allow": [...]}` per request — there is no persistent connection identity in the window today. So:
- The MCP bridge (`mailcraft-cli mcp --connect`) reads `clientInfo {name, version}` from `initialize` and adds it to every forwarded request as `"agent": {"allow": [...], "client": {"name", "version"}}`. Document the field in `docs/control-protocol.md` and `docs/mcp.md` (the protocol table's change log).
- The window treats it as **untrusted, self-reported text**: length-capped (64 chars), control/bidi characters stripped, shown as "reported as ‹name›". A request with an `agent` field but no `client` shows "Unnamed agent"; owner scripts (no `agent` field) never appear.
- **Never use the name for any decision** — any client can claim any name, and `--control-agents-only` already treats every request as an agent's.

**(b) Activity log (UI state, memory only; content per G12).** A ring buffer (e.g. 200 entries) in `MailApp`: time, reported client, command id, agent class, and one of these verdicts, each mapped 1:1 from the gate's code path so the owner can audit *who decided*:
- `ran — no question needed` (`Verdict::Run`, Safe class);
- `ran — allowed in advance (--allow <class>)`;
- `ran — you said yes`;
- `not run — you said no`;
- `not run — no answer in 50 s`;
- `refused` (Never class, `input` without `--allow input`, or agent access stopped). **No message text, no subjects, no addresses, no query strings** — only ids and counts (e.g. "search_mail: 7 results"). Not persisted to disk (keeps the privacy model simple; owner may later ask for persistence). **Agent-visible: no.** Mechanism: the ring buffer lives in a field of `MailApp` that the `ui.inspect` serializer (`control.rs`, which already emits `agentQuestion` at `:157`) does not read, and no MCP tool or control reply includes it. Test: an agent-classified `ui.inspect` reply after several logged requests contains no activity entry and no reported client name; the MCP tool list has no tool that returns it.

**(c) Agents panel (non-modal).** Title-bar **Agents** button (with a dot when anything ran in the last few minutes), opening a right-hand panel. **No "Waiting" section** (G11a): while a question is open the modal blocks the window and already says how many more are waiting, so a side list could never be acted on. Sections: *Recently active* (reported name, last seen, what it may do without asking — derived from the activity log, **not** a live connection list, because the window has no per-connection identity), *Activity*.

**Kill switch instead of per-agent Disconnect.** Because names are self-reported, "disconnect this agent" cannot be made truthful. Provide one owner control: **Stop agent access** (`ui.agentAccess {"value": false}`) — every request with an `agent` field is refused until the owner turns it back on, and queued questions are answered "no". Persistence per G15 (default: session only). The prototype's per-agent Disconnect is therefore **not** implemented; record this as a deviation from the proposal.

**(d) Unchanged on purpose:** the modal question, its bound yes, its 50 s timeout, default-No, Enter/Esc = no, input suppression, the queue limit of 8, `agentQuestion` in `ui.inspect`. Add a regression test for each of these before touching the code (characterisation tests), so a refactor that weakens one fails.

**Commands:** `ui.agents` (open panel), `ui.agentAccess {"value"?}` — both owner-only (`Never` for agents; an agent must never be able to turn agent access back on), classified by engine-engineer in `agent.rs`; extend `hostile_params_never_panic`.

**Tests:** fake control clients (no real mailbox): a request whose `agent.client.name` is 10 KB with RTL overrides and newlines → displayed truncated and sanitised; activity entries never contain fixture message text (assert on the log structure and on the panel's rendered strings); with agent access off, an agent request is refused and queued questions resolve "no"; an agent request for `ui.agentAccess` is refused; the owner's own control-channel script (no `agent` field) is unaffected; modal behaviour unchanged (the characterisation tests).

### 5.9 `refresh-boundary` [phase boundary] — lead, reviewers
- **Visual evidence:** **after** screenshots via `ui_shot.rs` for every surface in 5.0, in light/dark/HC and 200 %, at **1024, 1280 and 1440 px** window widths; the lead looks at each PNG (AGENTS.md rule). Pair before/after in the phase `evidence/`.
- **Accessibility:** for every new interactive element, a kittest test that finds it **by its accessible label** (`harness.get_by_label`, already used in this repo, e.g. `reading/warnings_tests.rs:150`) — this proves the AccessKit name exists. Keyboard via `Harness::key_press(egui::Key::Tab)` / `key_press(egui::Key::Escape)` (`egui_kittest/src/lib.rs:760`, verified in 0.37): Tab reaches the chip, ribbon AI group and Agents button; Esc closes the panel.
- **Contrast:** full `theme_contrast_tests.rs` matrix green.
- **Reviews:** security-privacy-reviewer and rust-quality-reviewer, read-only, once after implementation (team rule: reviewers dormant until the whole implementation is done, one batched correction cycle).
- **Gate:** `cargo xtask ci`, including wasm.

---

## 6. Waves, ownership and model routing

| Wave | Changes | Parallel? | Owners | Why this order |
|---|---|---|---|---|
| assess/analyze (no wave) | 5.0 deliverables | — | lead + ui-ux-designer | Gates first; nothing else is safe to write before them. Changes 5.1–5.9 are written at **plan**. |
| W1 | 5.1 | no | ui-engineer | Tokens are a shared dependency of 5.3, 5.4, 5.7. |
| W2 | 5.2, 5.3 | yes (disjoint files: `reading*` vs `mail_view.rs`/`chrome.rs`/`ribbon*`/`nav.rs`) | ui-engineer ×2 | Window track; no AI dependency. |
| W3 | 5.4, 5.5, 5.6, 5.7 | 5.4+5.7 together, 5.5 and 5.6 separately (5.5 touches `ribbon.rs` after W2; 5.6 touches `answers.rs`) | ui-engineer; engine-engineer for classifications | AI track; depends on committed AI code + 5.1. |
| W4 | 5.8 (a)→(b)→(c) | sequential | agent-surface-engineer → engine-engineer → ui-engineer | Identity before log before panel; characterisation tests first. |
| W5 | 5.9 | — | lead, reviewers | Boundary. |

**File-ownership grants** (put in `execution.md`):
- **`lib.rs` is integrator-owned.** Feature agents never edit it. Each wave ends with one integration task, owned by the lead (or a named integrator), that adds that wave's `UI_COMMANDS` entries, dispatch arms, `enabled` rules and panel placement from the hand-over notes each feature agent writes. This also covers 5.2's optional line-length setting (and its `backstage.rs` row), which keeps W2's two agents on disjoint files.
- `agent.rs` only by engine-engineer.
- `theme.rs` / `theme/*` only in W1 (5.1 owns all token values; later changes request a value from that owner rather than editing).
- `type_floor_tests.rs`: created and owned by 5.3 only; other changes call it from their own test files.
- `icons.rs`: 5.4 and 5.5 both draw icons — run them sequentially or give each a separate new file (`icons/ai.rs`) registered by the integrator.

**Task model assignments** (match the existing table format `Phase path | Change | Tasks | Role (route) | Why this model`): Opus 5.5 for the lead, 5.2 (layout/measure), 5.8 (safety-critical), and both reviewers; Sonnet 5.5 for 5.3, 5.4, 5.5, 5.7, icon drawing, i18n and docs fragments; Opus for 5.1 only if the contrast matrix forces token re-derivation.

---

## 7. Method notes for the KBD lead

- **Build cadence.** The AI phase used VLBV (no builds until the boundary). For this phase, recommend **allowing a build and screenshot per wave** for UI changes: constraint `ui-checked-visually` and the AGENTS.md "look at the result" rule cannot be met by reading code, and layout regressions (5.2) compound. Record this as a method decision at assess; if the owner keeps VLBV, move all visual checks to W5 and add a correction budget.
- **Adversarial review** at assess and plan (≤ 2 rounds each), through the maintainers' cross-model review gateway (judge ≠ producer). Use `--mode artifact` against the phase's `assessment.md`/`plan.md`.
- **Scenarios.** Freeze scenario IDs in the adopted contract as the AI phase did, e.g. `SCN-MEASURE-001` (paragraph ≤ measure, table full width), `SCN-CHIP-001` (chip state per provider kind), `SCN-AGENT-001` (hostile clientInfo sanitised), `SCN-AGENT-002` (log never contains message text), `SCN-AGENT-003` (modal properties unchanged), `SCN-A11Y-001` (new controls found by label), `SCN-CONTRAST-001` (matrix green).
- **Budgets.** Expect `reading/tests.rs` fixture churn in 5.2 and ribbon fit churn in 5.5; reserve one correction cycle for each.
- **Do not** edit `openspec/changes/ai-*` or the AI phase's artifacts; reference them.

---

## 8. Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Starting before AI code is committed → merge conflicts in `chrome.rs`, `lib.rs`, `ai/*` | High if ignored | Entry criteria (§4). |
| Measure cap breaks HTML newsletters / `@media` | Medium | Cap paragraph blocks only; tables, pictures and media evaluation keep full width; fixture tests. |
| New tokens fail HC 7:1 | Medium | Add to `PAIRS` first (test-first), then pick values; chip carries icon + words. |
| Agents panel weakens approval safety | Low–Medium, high impact | Modal untouched; characterisation tests; security review. |
| Spoofed agent names mislead the owner | Medium | "Reported as…", sanitised, never used for decisions; no per-agent Disconnect (would imply a guarantee we can't give) — one session-wide **Stop agent access** switch instead. |
| Activity log leaks mail content | Low, high impact | Ids/counts only; test greps for fixture text; memory only. |
| Ribbon AI group crowds Simplified ribbon | Medium | Fit test at 1024 px; group may start collapsed behind `…`. |
| Scope creep into a full type migration | Medium | G4a; touched surfaces only. |
| Phase artifacts published alongside `docs/design/` | Medium | Only `docs/design/` is public. Commit `plan/`, `openspec/` and `.kbd-orchestrator/` never; check `git status` before each commit. |

---

## 9. Definition of done (phase)

- Every gate answered and recorded.
- All changes archived; `openspec validate --strict` green.
- `cargo xtask ci` green (incl. wasm, i18n, layers, spdx, test-files).
- Before/after screenshots for every touched surface in light/dark/HC/200 %, looked at by the lead.
- New interactive elements reachable by keyboard and found by accessible label in tests.
- Agent modal properties proven unchanged by characterisation tests.
- Docs fragments written; commit trailers per AGENTS.md; owner accepts.

---

## 10. Sources consulted

- Codebase (this repo), 2026-11-17: files and lines cited in §1 and §5.
- AI phase artifacts: `.kbd-orchestrator/phases/agentic-ai-integration/{goals,plan,tasks,decision-log}.md`, child `agentic-ai-scenario-repair`, briefs in `plan/design/`.
- egui `TextWrapping` / `TextWrapMode::Truncate` and `Label::truncate` (docs.rs `egui`, emilk/egui#3244) — ellipsis behaviour, `max_rows` + `break_anywhere` guidance.
- `egui_kittest` (docs.rs 0.33.1 crate docs; repo pins 0.37): AccessKit-based queries (`get_by_label`), `Harness::render`, optional `snapshot` feature; egui's guideline to prefer non-image assertions over pixel snapshots.
- Agentic UX patterns, 2026 (figr.design "8 AI UX Patterns"; Towards AI "Designing for AI Agents"): proposal separated from action, review before consequential change, separate activity log.
- Email prompt-injection threat landscape (KuppingerCole, "When AI Agents Read Your Email"): every message is a potential injection; reading becomes an action problem once agents can send — supports keeping the bound, default-No modal and owner-only agent management.
- MCP `initialize` `clientInfo` (MCP specification) — self-reported, hence displayed as untrusted.

---

## 11. Adversarial review

Run with the `adversarial-review` skill in artifact mode through a local OpenAI-compatible review gateway. Gateway details were removed from the published review files. The judge was **mimo-v2.6-pro**; the plan was written by Claude Opus 5.5, so the reviewer is a different model family. Raw findings are in `docs/design/review/round1-findings.json` and `round2-findings.json`.

**Caveat, stated plainly:** the dispatcher recorded `cross_model_check: unverified-producer-unknown` and warned "canonical judge/producer identity is incomplete", even though `KBD_PRODUCER_MODEL` was set. The judge's family is clearly different, but the skill's own collision check did not certify it. The judge was not the configured `k3.1-pro`; the gateway's role resolution picked `mimo-v2.6-pro`.

| Round | Verdict | Critical | Warning | Suggestion | Screen |
|---|---|---|---|---|---|
| 1 (first draft) | BLOCK | 3 | 10 | 2 | — |
| 2 (revised) | PASS | 0 | 10 | 1 | anti-theater gate PASS (score 0.02) |

**Round 1 → what changed**
- *Proposal items silently dropped* → added the §4.1 coverage table; every item now has a change or an explicit cut. Widths verified now include 1280.
- *`ui.aiSettings` and other ids unverified* → verified in code and listed in §1.
- *`flagged_row` owned by two changes* → 5.1 owns token values; 5.3 owns rendering only.
- *"Waiting" list could never be acted on* → removed; new gate G11.
- *Log verdicts couldn't tell `--allow` from an owner yes* → six verdicts mapped 1:1 to the gate's code paths; `--allow` defined in §1.
- *Log content narrowed without asking* → new gate G12.
- *11 pt floor untested* → type-floor test helper.
- *Two ways to apply G4a* → new 11 pt rung only; global `Small` untouched.
- *Child task 6.6 not in entry criteria* → entry criteria name 6.4, 6.6 and 7.2–7.6.
- *Reviewer scheduled in two places* → reviewers only at 5.9.
- *G2 privacy test only for the rejected option* → test added for G2a.
- *`lib.rs` ownership unworkable* → integrator-owned, one integration task per wave.
- *Chip icons homeless* → owned by 5.4, with a test.
- *Flag+selection acceptance was "look at it"* → `row_fill` unit test for the four combinations.
- *5.0 looked like an OpenSpec change* → it's now assess/analyze deliverables.

Also changed on my own, not prompted by the judge: per-agent **Disconnect** became one session-wide **Stop agent access** switch, after finding that the control channel has no per-connection identity and MCP `clientInfo` isn't read today.

**Round 2 → what changed**
- *G4 wording contradicted 5.1* → reworded.
- *Coverage gaps (shared header column, To line, 12 px address, 3 px edge, accepted-query pills, AI-off message)* → each has a home, an assess check or an explicit cut in §4.1.
- *G5 smuggled in a summary row* → removed.
- *Type-floor mechanism unverified, reading pane missing* → mechanism verified in egui_kittest/epaint 0.37 and reading pane added; the helper lives in its own file, owned by 5.3.
- *Log invisibility to agents had no mechanism* → mechanism and test added.
- *Decisions hidden inside changes* → new gates G13–G15.
- *kittest APIs unverified* → `get_by_label` and `key_press` verified.
- *Re-plan contingency missed 5.5* → fixed.
- *Measure not reproducible* → defined as 72 × the width of '0' via the verified `glyph_width`.

**Not re-reviewed:** the round-2 fixes have not themselves been judged. The skill caps artifact review at two rounds. The KBD lead re-runs adversarial review on the phase's own `assessment.md` and `plan.md` anyway (§7), which will cover them.

---
---

# Part B — Phase `agent-runtimes`: multiple agentic modes, settings and sign-in

**Spec:** `AI-SETTINGS-SPEC.md` (normative for this part). **Research and sources:** `research/AGENT-RUNTIMES.md`. **Prototype:** `settings.html`. **Prompts:** `prompts/`.

## B0. TL;DR for the lead

1. **What the owner asked for:**
   - six agentic modes: Apple's on-device model first on Mac; OpenAI-compatible LLM providers; Claude Code detection and use; Codex detection and use; OpenCode detection and use; Universal Agent Runtime detection and use;
   - settings for all of them, with automated OAuth and token management, modelled on The Boss (`the-boss`).
2. **What the research changed (read research §0 first):**
   - **Claude:** Anthropic's terms forbid third-party apps routing requests through Pro/Max credentials. Claude Code is supported **with API billing only**; a subscription sign-in is detected and declined, with an explanation.
   - **ChatGPT plans:** usable by other apps only through **Sign in with ChatGPT (SIWC)**, which is open to open-source projects. A new **"ChatGPT plan" runtime** calls OpenAI's Responses API directly with SIWC tokens. Codex CLI is used with an **API key** only.
   - **The Boss's Codex sign-in** borrows the Codex CLI's OAuth client id. **Do not port it** (constraint `no-foreign-oauth-client-ids`).
   - **Tools:** Codex can't be made tool-free and OpenCode allows everything by default. Every CLI runtime gets a containment recipe and **kill-on-tool-event** (spec §7).
3. **Sequencing.**
   - Start after the AI phase closes (same entry criteria as Part A §4).
   - Run **after Part A**: both touch `ai/settings*` and the status chip. B10 re-points Part A's chip (A 5.4) from provider kinds to chain semantics.
   - If the owner wants B first, A 5.4, A 5.6 and A 5.7 must all be re-planned against the chain model, because B5.10 and B5.11 modify them (gate G-RT0).
4. **The hardest parts:**
   - the SIWC OAuth client and refresh logic (security-critical, and it needs an OpenAI client registration that MailCraft doesn't have yet);
   - containment: environment allow-list, process-group kill, stream parsers that detect tool events;
   - migrating `ai.json` v2 → v3 without losing consents or keys.

## B1. Entry criteria

- Part A's entry criteria (§4), plus Part A closed, or the owner chose G-RT0 (b).
- `ai.json` v2, `route.rs`, `settings_page.rs` and `settings_device.rs` are committed (they were mid-wave on 2026-11-17).
- **SIWC client registration** requested from OpenAI, or the owner accepts that B7 ships hidden behind "Not available yet" until it's granted (G-RT3).

## B2. What exists to build on (verified 2026-11-17)

| Area | Where | Reuse |
|---|---|---|
| Provider config, kinds (Local/Remote/Gateway), context windows, consents | `crates/ai/src/config.rs`; `apps/mailcraft/src/ai/settings.rs` (`ai.json` v2: primary, backup, `never_configured`, `on_device`, `keys_kept_for`) | Becomes the `compat:*` runtime and the base of v3 |
| Apple on-device via apfel, identity check, confirm-once binding | `crates/ai/src/on_device.rs`; `apps/mailcraft/src/ai/device.rs`; `ui-egui/src/ai/settings_device.rs` | Becomes the `on_device` runtime |
| Streaming HTTP, SSE parser, loopback enforcement, TLS-only | `crates/net` (`sse.rs`, fuzzed) | SIWC, UAR, compat |
| Request thread, cancel, `Host::run_job` | `apps/mailcraft/src/ai/run.rs` | All runtimes |
| Keys in OS credential store, redacted `ApiKey` | `apps/mailcraft/src/ai/keys.rs`; `crates/ai/src/key.rs` | API keys, SIWC tokens |
| Primary/backup routing, fallback chain (child task 5.3) | `apps/mailcraft/src/ai/route.rs`, `settings_chain_tests.rs` | Generalised to the N-entry chain |
| Owner-only control methods `ai.providers`, `ai.setProvider`, `ai.setConsent`… | `crates/ui-egui/src/control/ai.rs`; refused to agents at `agent_gate.rs:99` | Extended per spec §10 |
| "Answered by" and fallback notice (child task 6.6) | answer pane | Extended with the failed-runtime list |

## B3. Decision gates — settle at assess with the owner

| Gate | Question | Options | Recommendation |
|---|---|---|---|
| **G-RT0 Order** | Part A or Part B first? | (a) A then B; (b) B then A, re-planning A 5.4, 5.6 and 5.7 against the chain | **(a)** A is smaller and mostly visual; B then updates A's chip |
| **G-RT1 On-device default** | Make Apple's model first in the chain on Macs that have it? | (a) one-time yes at first run, default Yes; (b) silent default; (c) off until chosen | **(a)** Local and private, but still the owner's choice. Matches the existing confirm-once rule |
| **G-RT2 Claude subscriptions** | The owner wants Claude Code; Anthropic forbids third-party use of Pro/Max | (a) API billing only, decline subscription sign-ins with an explanation; (b) allow the owner's own subscription sign-in anyway | **(a)** The policy text is explicit (research §0.1). (b) would put a public open-source project in breach and could get users' accounts limited. Record the owner's decision verbatim |
| **G-RT3 SIWC registration** | Who registers MailCraft with OpenAI, and what ships before approval? | (a) the owner registers; B7 code ships, runtime hidden until a client id is configured; (b) defer B7 | **(a)** |
| **G-RT4 Codex** | Include Codex given its shell tool can't be removed? | (a) include, off by default, API key only, kill-on-tool, warning; (b) defer | **(a)**: the owner asked for it; the containment recipe plus kill-on-tool bounds the risk |
| **G-RT5 OpenCode** | Include now? | (a) include with deny-all config; (b) defer until its V2 schema is verified | **(a)** if assess verifies the deny-all config path and `run --format json`; else **(b)** |
| **G-RT6 Codex home** | Run Codex with the owner's `CODEX_HOME` or a MailCraft-owned one? | (a) MailCraft-owned (API key via `CODEX_API_KEY`); (b) the owner's | **(a)** The owner's ChatGPT login is never used by accident |
| **G-RT7 UAR** | Bundle `uar-sidecar` or detect and start an installed one? Managed or external instance? | (a) detect + start installed, managed by default, external opt-in with a warning; (b) bundle | **(a)** Bundling adds an asset/licence/update burden (ATTRIBUTION.md) |
| **G-RT8 Starting CLI sign-in** | Spawn the login command directly or open the owner's terminal? | (a) spawn when non-interactive works, else terminal with a fixed argv; (b) always tell the owner the command | **(a)** |
| **G-RT9 Chain replaces primary/backup** | Accept the v3 model and migration? | (a) yes; (b) keep primary/backup and add CLI runtimes as backups | **(a)** It generalises what exists; the migration table is in spec §3.1 |
| **G-RT10 Fallback default** | `next`, `ask` or `stop`? | — | **`next`**, but a policy kill never falls through silently |
| **G-RT12 Codex sign-in automation** | The owner asked for automated OAuth and token management for Codex, modelled on The Boss. The Boss does it by borrowing the Codex CLI's OAuth client id, which is forbidden here. Codex with ChatGPT is also not a sanctioned plan route. | (a) Codex with an API key only; the ChatGPT plan is automated through SIWC (B5.7) instead; (b) drive Codex's own `codex login` and use whatever it signs in with | **(a)**. Record the owner's answer verbatim: this narrows the original request. |
| **G-RT11 Direct Anthropic API** | A direct Messages API runtime would be a tool-free Claude path. It is **not** an OpenAI-compatible `compat` provider (different wire format), so it would be its own adapter (module, credential entry `ai.anthropic_api`, `Where` = `leaves`, fuzz target, scenarios) | (a) defer to a later phase; (b) add as B5.14 now | **(a)** Keep this phase's scope; Claude Code with an API key covers the request |
| **G-RT13 Safe containment crates** | Process groups, job objects and sandboxes must be built in safe Rust (`unsafe_code = "forbid"`) | (a) confirm at assess: `process_group(0)` + `nix::killpg`; a safe job-object crate; `sandbox-exec` / Landlock launcher / `bwrap` via argv (spec §7.1); (b) the owner allows a reviewed `unsafe` carve-out | **(a)**; resolve before W2 |

## B4. Phase definition

**Name:** `agent-runtimes`.

**Goals (proposed `goals.md`):**
- Follow the KBD process in order (assess → analyze → plan → execute → reflect).
- Answers can come from any of:
  - Apple's on-device model;
  - OpenAI-compatible providers;
  - a ChatGPT plan through Sign in with ChatGPT;
  - Claude Code (API billing);
  - Codex (API key);
  - OpenCode;
  - the Universal Agent Runtime.

  All are tried in an owner-ordered chain, with a stated fallback.
- Every runtime is used as a text model. Any attempt to use a tool stops the run and tells the owner.
- MailCraft detects installed runtimes and shows their version and status.
- MailCraft runs Sign in with ChatGPT itself. It starts Claude Code's own sign-in only for an API-billed Console account, and it never starts Codex's (G-RT12). It keeps its own secrets only in the OS credential store (access tokens in memory only) and never reads a CLI's.
- Anything that leaves the computer needs a yes per account. Claude subscriptions are never used.
- Hold every AGENTS.md rule; wasm keeps building.

**Out of scope:** tool use on mail; installing or updating CLIs; bundling `uar-sidecar` (G-RT7); ACP (future adapter); Android/iOS; AI-drafted replies (Part A G10).

## B5. Changes (OpenSpec)

Each change has `proposal.md`, `design.md`, `tasks.md` and `specs/<capability>/spec.md`, and passes `openspec validate --strict`. Spec sections are in brackets.

| # | Change id | Capability | What | Owners | Spec |
|---|---|---|---|---|---|
| B5.1 | `ai-runtime-model` | ai-runtimes | Runtime ids, `Where`, chain types, consent-per-runtime in `crates/model`; `ai.json` v3 read/write + v2 migration + `ai.v2.json` keep in host `settings.rs` | engine-engineer (model), ai-provider-engineer (host) | §2, §3 |
| B5.2 | `ai-runtime-commands` | ai-runtimes | `ai.runtimes`, `ai.setRuntimeOrder`, `ai.setRuntimeEnabled`, `ai.setRuntimeOption`, `ai.rescan`, `ai.signIn`, `ai.signOut`, `ai.setRuntimeKey`, `ai.setFallback`; all `Never` for agents; `hostile_params_never_panic` | engine-engineer, agent-surface-engineer (control/ai.rs) | §10 |
| B5.3 | `ai-runtime-detection` | ai-runtime-detection | Login-shell PATH, candidate walk, `--version` probe, writable-binary refusal, cache, Look Again | ai-provider-engineer | §5 |
| B5.4 | `ai-runtime-containment` | ai-runtime-containment | Common spawner: argv without prompt or secrets, empty 0700 cwd, per-run `HOME`, env allow-list, caps, timeouts, process-group kill, spawn-time executable checks, **OS sandbox profiles (Seatbelt / Landlock / AppContainer)**; tool-event detector interface; shared fuzz harness (each adapter adds its own parser's fuzz target) | ai-provider-engineer, security review at boundary | §7.1 |
| B5.5 | `ai-runtime-claude-code` | ai-runtime-claude-code | Recipe §7.2; `claude auth status` parser + allow-list (decline subscription); key handoff via env; start `claude auth login --console` + poll | ai-provider-engineer | §6.3, §6.4, §7.2 |
| B5.6 | `ai-runtime-codex` | ai-runtime-codex | Recipe §7.3; `CODEX_API_KEY` from keychain; per-run `CODEX_HOME` and `HOME` (G-RT6, G-RT12); refuses to run without an OS sandbox; kill on every tool class; JSONL parser + fuzz target | ai-provider-engineer | §6.3, §7.3 |
| B5.7 | `ai-runtime-chatgpt-siwc` | ai-runtime-chatgpt | PKCE loopback OAuth, **refresh token only** in the credential store (access token in memory), refresh with compare-and-set, terminal-4xx handling, sign-out/revoke, Responses API streaming without tools; hidden until a client id is configured (G-RT3) | ai-provider-engineer; security-privacy-reviewer at boundary | §6.2, §7.5 |
| B5.8 | `ai-runtime-opencode` | ai-runtime-opencode | Deny-all config, `run --format json` (or `serve` + basic auth), `auth list` status parser, terminal sign-in launcher (G-RT5) | ai-provider-engineer | §6.3, §6.4, §7.4 |
| B5.9 | `ai-runtime-uar` | ai-runtime-uar | Managed sidecar lifecycle (stdin token, `READY:<port>`, bearer, generation counter, stdin-close shutdown), discovery on 1906, external-instance opt-in, AG-UI text-event consumer, tool-event cancel | ai-provider-engineer, agent-surface-engineer (AG-UI types) | §7.6 |
| B5.10 | `ai-runtime-chain` | ai-runtimes | `route.rs` → N-entry chain; per-request account set; consent and status checks per hop; fallback policy; provenance list in the answer pane (extends child task 6.6) | ai-provider-engineer, ui-engineer (provenance text) | §9 |
| B5.11 | `ai-runtime-settings-ui` | ai-runtime-settings | AI assistance page per `settings.html`: chain list with arrows, status/where tags, details panel, sign-in dialogs, key dialog, accounts matrix, fallback radio; status chip update (Part A 5.4); narrowing badge names the runtime (Part A 5.7, SCN-RT-BADGE-001) | ui-ux-designer (brief), ui-engineer | §11 |
| B5.12 | `ai-runtime-docs` | — | PRIVACY.md, threat-model (incl. the text-exfiltration residual risk), docs/log.d + release-notes fragments, docs/mcp.md (agents still can't use these), and **`docs/design/prompts/` (`README.md`, `00-preflight.md` … `07-boundary-and-reflect.md`) brought in line with the recorded gate answers and final change and scenario ids** | architect | §12 |
| B5.13 | `ai-runtimes-boundary` | — | Scenarios §13, screenshots, reviews | lead, reviewers | §13 |

## B6. Waves and ownership

| Wave | Changes | Parallel | Notes |
|---|---|---|---|
| assess/analyze | gates, verification list (research §11), brief for B5.11 | — | **Entry gates for W2/W3:** `claude --system-prompt-file` and `--setting-sources ""`; the `claude auth status` billing field (decides whether Console sign-in stays); OpenCode stdin/`serve` message path (decides G-RT5); safe crates per platform (G-RT13). B5.4, B5.5 and B5.8 are written only after these are recorded |
| W1 | B5.1, B5.2 | B5.1 model ∥ B5.2 command specs (engine-engineer owns `agent.rs`) | Shared types first |
| W2 | B5.3, B5.4 | yes (detection vs spawner are separate host modules) | |
| W3 | B5.5, B5.6, B5.7, B5.8, B5.9 | yes: one file set per adapter under `crates/ai/src/runtime/<name>.rs` + `apps/mailcraft/src/ai/runtime/<name>.rs` | The integrator registers adapters in one registry file |
| W4 | B5.10, B5.11 | B5.10 then B5.11's provenance; settings page in parallel | `lib.rs` integrator-owned (Part A §6 rule) |
| W5 | B5.12, B5.13 | — | Reviews dormant until here |

**Grants:**
- `crates/ai/src/runtime/mod.rs` (adapter registry) and `lib.rs` are integrator-owned.
- `agent.rs` belongs to engine-engineer.
- `settings.rs` (v3) belongs to B5.1 only. Later changes request fields from B5.1's owner.

**Model routing:**
- Opus 5.5: lead, B5.4, B5.7, B5.9, B5.10 and both reviewers. These are security- or concurrency-critical.
- Sonnet 5.5: B5.3, B5.5, B5.6, B5.8, B5.11 widgets, B5.12.

## B7. Method notes

- **Build cadence:** same recommendation as Part A §7 (build per wave). B5.4's process-group handling and B5.7's OAuth need real runs against fakes early.
- **Adversarial review:** at assess and plan, ≤ 2 rounds each, cross-model, as in §7.
- **Fakes, not real accounts:**
  - Every scenario except SCN-RT-LIVE-001 uses test-written fake CLIs (shell scripts that print fixture JSON), a fake OAuth server on loopback, a fake credential store, and a fake UAR sidecar.
  - SCN-RT-LIVE-001 is owner-run with their own credentials, never in CI.
- **Do not port The Boss's code.** The Boss is TypeScript and MailCraft is Rust-only, and several of its patterns are explicitly rejected (research §10). Port the *patterns* in research §9.

## B8. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| The owner wants their Claude Max subscription regardless | Medium | G-RT2 records the decision; the spec's default declines it, and the UI explains why with the policy link |
| SIWC registration not granted or delayed | Medium | B5.7 ships hidden; the ChatGPT plan is reachable meanwhile via OpenCode (a SIWC partner) if G-RT5 is (a) |
| A CLI changes flags or event schema (they release weekly) | High | Version-gated recipes; parsers tolerate unknown events but never unknown *tool* events (unknown event kinds that aren't text ⇒ kill); a nightly owner-run smoke test (SCN-RT-LIVE-001) |
| Prompt injection makes a coding agent run a command | Medium (Codex) | Sandbox prevents most effects; kill-on-tool stops the run; empty cwd; env allow-list; Codex off by default |
| Secrets or mail leak through env inheritance or argv | Low, high impact | Env allow-list (SCN-RT-CONTAIN-002); prompt on stdin, instructions in a 0600 file, no argv fallback anywhere (SCN-RT-ARGV-001) |
| A runtime reads a readable file and sends its text to the vendor before any tool event arrives | Medium, high impact | OS sandbox limits reads (SCN-RT-SANDBOX-001); per-run `HOME`; Codex and OpenCode refuse to run without a sandbox; the residual risk is named in the threat model and PRIVACY.md |
| v2 → v3 migration loses consents or keys | Medium | Migration table tests (SCN-RT-MIGRATE-001); `ai.v2.json` kept one release |
| GUI app can't see the shell PATH on macOS | High (known) | Login-shell PATH probe (spec §5.1) |
| UAR external instance has tools we can't disable | Medium | External is opt-in, labelled, and counts as `leaves` |

## B9. Definition of done

- Every B3 gate answered and recorded (decision-log).
- All B5 changes archived; `openspec validate --strict` and `cargo xtask ci` (incl. wasm) green.
- Scenarios SCN-RT-* (spec §13) pass. The owner-gated SCN-RT-PRIV-001 and SCN-RT-LIVE-001 are run by the owner, or recorded as not run.
- B5.12 archived, including the update of `docs/design/prompts/`.
- Screenshots of the AI assistance page in light, dark, high contrast and 200 % for: fresh Mac; everything detected; subscription declined; sign-in waiting; accounts matrix.
- PRIVACY.md and threat-model updated; release-note fragments written; owner accepts.

## B10. Effect on Part A

- A 5.4 (status chip) reads chain semantics after B. Its states become:
  - "AI off" (no runtime enabled);
  - "AI: on this computer" (every enabled runtime is local);
  - "AI: may leave for ‹accounts›" (any runtime that leaves has consent).

  B5.11 owns this update.
- A 5.6 provenance gains the failed-runtime list (B5.10).
- A 5.7's narrowing badge names the runtime ("On this computer" / "ChatGPT plan" / "Claude Code" …). B5.11 owns this. SCN-RT-CHIP-001 covers the chip states.

## B11. Adversarial review of Part B

The review ran cross-model through a local OpenAI-compatible gateway; the judge was **mimo-v2.6-pro** and the plan was written by Claude Opus 5.5. The first attempt timed out three times on a 78 KB packet. It was rerun without the research note and with the constraints truncated to 6 KB; that limitation is recorded in the packet. Raw findings are in `review/round3-findings.json` and `review/round4-findings.json`.

| Round | Verdict | Critical | Warning | Suggestion |
|---|---|---|---|---|
| 3 (Part B first draft) | BLOCK | 6 | 9 | 1 |
| 4 (revised) | BLOCK | 2 | 9 | 1 |

**Round 3 → fixed:**
- *Prompt and mail on argv* → stdin plus a 0600 instructions file; no argv fallback; SCN-RT-ARGV-001.
- *`Where` undefined for three runtimes* → fixed per runtime type, never self-reported; SCN-RT-WHERE-001.
- *UAR self-reported local* → MailCraft-verified loopback model only.
- *Kill-on-tool is detective only* → OS sandbox per runtime, per-run `HOME`, Codex and OpenCode refuse to run without a sandbox, residual risk documented; SCN-RT-SANDBOX-001.
- *Tool-class coverage* → a fixture per class plus an unknown kind, with a grandchild heartbeat oracle.
- *Stored access token vs the constraint* → refresh token only, access token in memory.
- *Fuzz targets in the wrong wave* → moved into each adapter.
- *A 5.7 badge unowned* → B5.11; SCN-RT-BADGE-001.
- *Migration gaps* → rows for credential entries and the backup.
- *`CODEX_HOME` ambiguity* → per-run, never persisted.
- *SCN-RT-PRIV-001* → owner-gated, with a stated capture method.
- *Prompts unowned* → B5.12.
- *Codex OAuth narrowing* → gate G-RT12.
- *Path overrides* → re-checked at spawn; SCN-RT-PATH-001.
- *"Six" vs seven runtimes* → fixed.

**Round 4 → fixed after the round** (the skill allows two rounds, so **these fixes were not re-reviewed**; the lead's own adversarial review of the phase's `plan.md` covers them):
- *Claude Console sign-in could never succeed* → the predicate uses the status's billing field, verified at assess; if no such field exists, Console sign-in is dropped (API key only).
- *Any 4xx killed the ChatGPT sign-in* → terminal only for `invalid_grant` / `invalid_client` / `unauthorized_client` / revocation; 408, 429 and 5xx retry; negative controls added.
- *Client-id delivery* → a build-time constant only; hidden when absent.
- *G-RT11 underspecified* → recommendation changed to defer, spelled out as its own adapter.
- *Prompts ownership* → B5.12 names the files.
- *Chip and `ask`/`stop` untested* → SCN-RT-CHIP-001 and SCN-RT-CHAIN-002.
- *G-RT0 (b) incomplete* → it now covers A 5.4, 5.6 and 5.7.
- *Migration precedence* → explicit rows for all four combinations.
- *Hostile v3 file* → SCN-RT-SETTINGS-001.
- *`unsafe` vs process groups and sandboxes* → safe-crate plan in spec §7.1 and gate G-RT13.
- *Unverified flags frozen into recipes* → listed as entry gates for W2/W3 in §B6.
