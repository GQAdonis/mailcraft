# 2 — Assess

> **Run once already** for phase `mailcraft-ui-refresh-and-agent-runtimes`: its gate answers, final change ids and frozen scenario ids are in [README.md](README.md#the-record-of-the-phase-these-prompts-ran). OpenCode is cut (G-RT5 b).

Run `/kbd-assess` for `<PHASE>`, including its multi-model preflight. In addition to the skill's own steps:

1. **Re-verify the code inventory.** For Part A that's PLAN.md §1; for Part B, §B2. Check every file:line against HEAD and record any drift in `assessment.md`. Line numbers will have moved.
2. **Verification list.**
   - **Part A:** run PLAN.md §4.1's three assess checks: focus ring coverage, empty states, list keys. Also check the "To" line ellipsis and the accepted-query pills.
   - **Part B:** run every item in `research/AGENT-RUNTIMES.md` §11 against the **installed** versions on this machine and the vendors' current docs:
     - Run `claude --version`, `claude auth status --json`, `codex --version`, `~/.uar/bin/universal-agent-runtime --help`, and the UAR agent-card probe (`/.well-known/agent-card.json` on 127.0.0.1:1906). (**Amended:** OpenCode is cut, G-RT5 b; Codex's status is never probed with `codex login status`, since MailCraft runs no `login` subcommand.)
     - **Only** read status. Never print or read credential files.
     - Re-check Anthropic's and OpenAI's policy pages quoted in research §0. If the wording changed, quote the new text.
3. **Gap list.** For each goal, write what exists, what's missing, and the risk.
4. **Draft the decision batch.**
   - Part A: §3 gates G1–G18.
   - Part B: §B3 gates G-RT0–G-RT13.
   - Update each recommendation if verification changed the facts. For example, OpenCode's deny-all config path couldn't be confirmed, so deferring it (G-RT5 b) was recommended and accepted.
5. **Adversarial review** of `assessment.md`: artifact mode, at most 2 rounds. Record the results in the phase's `review/`.
6. Stop and hand off to prompt 3.
