# 3 — Owner decisions

1. Present every open gate from `assessment.md` to the owner **in one batch**. Use a question form; each gate gets a one-line summary, the options, and the recommendation pre-selected.
   - Part A: G1–G18.
   - Part B: G-RT0–G-RT13.
2. **Gates that need the owner's own words. Do not accept a default for these:**
   - **G-RT2 (Claude subscriptions).** Show the quoted policy text from `research/AGENT-RUNTIMES.md` §0.1, or the updated text found at assess. Record the owner's answer verbatim. *Recorded: b, the owner's own subscription in the unmodified CLI; MailCraft never reads the login.*
   - **G-RT3 (SIWC registration).** Who registers MailCraft with OpenAI, and when. *Recorded: a, OpenAI's dynamic per-account registration, so nobody registers a client id.*
   - **G-RT12 (Codex sign-in).** *Recorded, owner-specified:* Codex runs on the owner's own ChatGPT login in the owner's `CODEX_HOME` (with G-RT6 b); MailCraft never runs `codex login`, never reads `auth.json`, and holds no Codex key. The earlier recommendation (an API key only) was overturned by the owner's "no per-token billing, ever".
   - **G7 / G11 / G12 (agent approvals and log)** in Part A. They touch the safety model.
3. Record every answer in `decision-log.md` with the date. Mark any gate the owner didn't answer as "lead default, owner may overturn".
4. If an answer changes the scope, update the phase `goals.md` (out-of-scope list) and note it in `decision-log.md`. For example, G-RT2 (b) or G-RT5 (b).
5. Stop.
