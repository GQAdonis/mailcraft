# MailCraft: interface improvements and AI integration — design proposal

Companion to the clickable prototype in `index.html`. Everything here continues MailCraft's own design language: the light and dark palettes in `crates/ui-egui/src/theme.rs`, Inter (bundled), the blue accent, and the Outlook layout. Nothing in the MailCraft repository was changed.

People, companies, addresses and model answers in the prototype are fictional. The AI responses are scripted to show the flows; no model is called.

---

## Part 1 — Fixes to the current window

| # | Issue in the current window | Proposed change | Where to see it |
|---|---|---|---|
| 1 | Reading-pane body runs ~150 characters per line on wide windows | Body text capped at **72ch**, 15 px, line height 1.65. Header and attachments share the same column | Open any message |
| 2 | `Maya Okafor<maya@…>`: no space, address in the same weight | Name in semibold, address after a gap in the dim colour at 12 px; "To" on its own line, truncated with an ellipsis | Sender block |
| 3 | Report a Problem and Copy Diagnostics are full buttons in the title bar | Moved to **Help** (Report a Problem…, Copy Diagnostics, About). Title bar is now: mark, sync/undo/redo, search, **Agents**, account | Help tab |
| 4 | Flagged rows are as loud as selection (pink / maroon fill) | Flag tint lowered to a near-white wash (`#FEF7F8` light, `#2A2224` dark) plus a red flag glyph. Selection (fill + 3 px accent edge) always wins | "RE: Release checklist" row |
| 5 | Previews cut mid-word with no ellipsis | Sender, subject and preview each truncate with an ellipsis on their own line | Message list |
| 6 | 12.5 / 10.5 pt type is small; 10.5 in the dim colour is hard to read | Scale is 11 / 12 / **13** / 15 / 16 / 20 px. Nothing smaller than 11 px; group headings are 11 px semibold uppercase with letter-spacing | Throughout |
| 7 | Uniform 6 px radius everywhere; 6×4 and 8×3 spacing off-grid | Radii by role: control 6, surface 8, window 12, pill full. Spacing strictly 4 / 8 / 12 / 16 / 24 / 32 / 48 | Throughout |
| 8 | Disabled "Send/Receive All Folders" occupies a large ribbon slot | Rendered as a small button in its group, beside Work Offline | Ribbon, far right |
| 9 | Module bar is icons only | The bar can be widened to show labels (Mail, Calendar, People, To Do); the active module keeps the accent edge | Bottom of the module bar |

Also applied: one focus ring for every control (2 px accent, 2 px offset), honest empty states ("Nothing in Sent Items…", "No mail matches…"), and keyboard movement through the list with ↑/↓ and Enter.

### Responsive behaviour
- **≥ 1280 px:** module bar · folder pane · list · reading pane.
- **1024–1279 px:** the folder pane becomes an overlay opened from the list header.
- **768–1023 px:** the module bar narrows and loses its label option.
- **< 768 px (browser build on a phone):** the module bar moves to the bottom; list and reading pane become two screens with a Back control.

---

## Part 2 — AI and agent features

These follow the open change proposals in `openspec/changes/` (`ai-narrow-search`, `ai-cited-answers`, `ai-provider-layer`, `ai-search-mcp-tool`) and the product principles in `PRODUCT.md`: local first, the owner decides, and mail is data, never instructions.

### 2.1 One box, two modes: Search and Ask
The title-bar search gains a two-way switch, **Search | Ask**, inside the field. Typing operators (`from:`, `has:`) still searches immediately, exactly as today. `Ctrl/⌘ K` jumps straight to Ask.

*Why one box:* the search box is where people already go to find mail. A second entry point would split that habit. The switch makes the mode explicit, so nobody is surprised by an AI answer when they wanted a list.

### 2.2 Plain-words narrowing (`ai.narrow`)
Typing a sentence in Search mode, e.g. *invoices from Cloudline last month*, opens **Search for this?** under the box:

- what you typed, quoted;
- the proposed query as removable pills (`invoice` `from:cloudline` `after:2026-09-01`), in monospace so they read as query language;
- a badge saying where the proposal was made (**On this computer**, or **Remote: host** in amber when that account allows a remote model);
- **Run search** (primary), **Edit as text**, **Cancel**.

The list doesn't change until you choose Run search, which is the owner's decision recorded in the proposal. The accepted query stays visible as pills above the list, with **Clear**. With AI off, the words are searched as typed and a short message says so.

### 2.3 Cited answers (`ai.ask`)
In Ask mode, a question opens an **answer view in the reading pane**. A side panel would squeeze the list.

- The answer streams in, with a **Stop** button (Esc also stops). Stopping keeps the part already written.
- Every claim carries a numbered **citation chip**; a **Sources** list repeats them with subject, sender and time. A chip opens the message, and **Back to the answer** returns.
- A **provenance** block says which model answered, where it ran, how many messages it read, and that their text was treated as data.
- Suggested follow-up questions sit at the end.
- If the answer is incomplete, it says so in plain words (for example, "Mail from your personal account wasn't searched").
- With AI off, the view says nothing was asked and offers **Set up AI**.

### 2.4 Suggested drafts (exploratory)
Reply has **Suggest a draft** (also **Suggest Reply** in the ribbon). The suggestion streams into the normal reply box on a pale blue wash with a blue left edge and the note *"Suggested by your model on this computer. Edit it freely; nothing is sent until you choose Send."* Once you type, the wash disappears and the note reads *"Suggested draft, edited by you."*

This is marked **exploratory** because `PRODUCT.md` still lists AI-assisted compose as an open decision.

### 2.5 Where AI runs: settings and consent (`ai-provider-layer`)
**File › Options › AI and agents** (also the **AI Settings** ribbon button and the status-bar chip):

- **Where AI runs:** Off, or On this computer (the default when set up). The local model address only accepts `localhost` / `127.0.0.1`, and an error explains why when it isn't.
- **Remote models, account by account:** each account shows its state in plain words and an **Allow…** button. Allowed accounts show the provider host and a red **Revoke**.
- **Consent dialog** for one account, laid out as three facts:
  - **What is sent:** the text of the few messages that match, with subject, sender and date; never attachments, passwords or other accounts.
  - **To whom:** only the provider entered, over an encrypted connection.
  - **How to stop:** revoke at any time; requests stop at once, and what was already sent can't be recalled.
  
  The provider address must be `https://` (checked when the field loses focus, with the remedy in the error), and **Allow for this account** stays disabled until "I understand…" is ticked.
- **Agents:** "Ask me before an agent sends, deletes or changes an account" is shown checked and locked, labelled *Always on: an agent can't answer for you.*

### 2.6 Always-visible AI status
The status bar gains a chip next to "This mailbox is on this device only":

| State | Chip |
|---|---|
| Off | grey, "AI off" |
| Local only | green, "AI: on this computer" |
| Any remote allowed | amber, "AI: remote allowed for Work" |

It opens the AI settings. Colour is never the only signal: each state has its own icon and words.

### 2.7 Agents: the owner decides
An **Agents** button in the title bar, with a count when requests are waiting, opens a drawer with three sections:

1. **Waiting for you:** one card per request, showing exactly what the agent wants to do (recipient, subject and full body for a send; sender and subject for a delete). Each card has **Send it / Delete it** (delete uses the danger style) and **Decline**. The note under the buttons says it allows this one request only.
2. **Connected:** each MCP client, with what it can do without asking (search and read) and **Disconnect**.
3. **Activity:** a timestamped log of what agents searched, read and asked for, and what you decided.

This makes the existing agent gate (`agent_gate.rs`) visible, and it is where the `search_mail` MCP tool's activity shows up.

---

## Part 3 — Tokens added for these surfaces

All are derived from the existing palette. Proposed names follow `Tokens` in `theme.rs`.

| Token | Light | Dark | Use |
|---|---|---|---|
| `flagged_row` (changed) | `#FEF7F8` | `#2A2224` | quieter flag wash |
| `cite` | `#E3EDFF` | `#263451` | citation chips (= `accent_soft`) |
| `suggest_wash` / `suggest_edge` | `#F3F7FF` / `#8FB4F5` | `#1F2636` / `#3F67A8` | suggested draft |
| `ai_local` / `ai_local_soft` | `#1F7A4D` / `#E6F4EC` | `#5BC08A` / `#1F3328` | "on this computer" (= `green`) |
| `ai_remote` / `ai_remote_soft` | `#9C6700` / `#FFF6E0` | `#E8B74A` / `#3A3020` | remote allowed (= `icon_yellow`) |
| `danger_soft` | `#FDECEC` | `#3A2424` | delete requests, Revoke hover |

Type scale: `xs 11 · sm 12 · md 13 · read 15 · lg 16 · xl 20`. Motion stays within 150–220 ms with an ease-out on entry. Animations follow File › Options › Animations and the system's reduced-motion setting; with animations off, answers and drafts appear whole instead of streaming.

---

## Open questions for the team

1. **Ask placement:** the reading pane (proposed) or a dockable side pane for long research sessions?
2. **Draft suggestions:** in scope for this phase, or wait for the compose decision?
3. **Remote consent scope:** per account (proposed, matching `ai-provider-layer`) or per account *and* per feature (asking vs. drafting)?
4. **Agent approvals while the window is minimised:** a system notification, or only the badge?
5. **High contrast:** the new washes (suggestion, flag, AI chips) need high-contrast variants at 7:1, as the theme does for messages today.
