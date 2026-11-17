# Agent runtimes for MailCraft — research note

**Checked:** 2026-11-17. Versions and policies change fast; re-check every "verify" item at the new phase's **assess** stage.
**Scope:** how a native Rust mail client (no Node or Python inside the app; it may spawn subprocesses) can use six agent runtimes **as text models**. Mail is untrusted input, so a runtime that can run tools is a prompt-injection path to the owner's machine.
**Sources:** vendor documentation (cited inline) and a code reading of *The Boss* (`the-boss`, a Cherry Studio fork), cited as `the-boss:<path>:<line>`.

---

## 0. The findings that change the design

1. **Claude subscriptions can't be used by other apps.** Anthropic's Claude Code legal and compliance page says OAuth sign-in is "designed to support ordinary use of Claude Code and other native Anthropic applications", that developers "should use API key authentication", and that "Anthropic does not permit third-party developers to … route requests through Free, Pro, Max, or Ultra plan credentials on behalf of their users" and "may not collect, store, or intermediate Claude.ai credentials or session tokens" ([code.claude.com/docs/en/legal-and-compliance](https://code.claude.com/docs/en/legal-and-compliance); same in [agent-sdk/overview](https://code.claude.com/docs/en/agent-sdk/overview)). OpenCode removed its Claude Pro/Max sign-in after legal requests ([anomalyco/opencode#18186](https://github.com/anomalyco/opencode/pull/18186)).
   → **MailCraft uses Claude Code only with API billing**: an API key, or Claude Code signed in with a Console account. It detects a subscription sign-in and declines it, with a clear message.
2. **ChatGPT plans have a sanctioned route: "Sign in with ChatGPT" (SIWC).** OpenAI Help: "you can also choose to use your ChatGPT plan for AI requests in participating commercial apps and sites… Supported open-source tools remain available to all ChatGPT users" ([help.openai.com/…/20001542](https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites)). Developer docs: "ChatGPT plan usage is available to open-source projects, personal projects that run locally, and selected private apps" ([cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt); [quickstart](https://developers.openai.com/siwc/quickstart)).
   → **MailCraft adds a "ChatGPT plan" runtime that uses SIWC directly** (its own OAuth client, registered with OpenAI), calling the Responses API with no tools. This, not driving the Codex CLI, is how "use my ChatGPT plan" works.
3. **Don't copy The Boss's Codex sign-in.** It uses the Codex CLI's OAuth client id (`app_EMoamEEZ73f0CkXaXp7hrann`), its fixed callback port 1455 and `codex_cli_simplified_flow`, then calls `chatgpt.com/backend-api/codex/responses` with the plan token (`the-boss:src/main/services/oauth/providers/codex.ts`, `src/main/ai/provider/codex.ts:50`). That impersonates another program. MailCraft's rule "never ship another program's OAuth id" (constraint `no-foreign-oauth-client-ids`) already forbids it.
4. **The coding agents can't all be made tool-free.**
   - Claude Code: yes, with flags.
   - Codex: no documented flag removes its shell tool. Read-only still allows read commands.
   - OpenCode: allows everything by default, but can deny everything by config.
   - UAR: disables its native tools when told to.

   → Each adapter carries its own **containment recipe** (§2–§6), and Codex is off by default with a warning.
5. **"Universal Agent Runtime" is Prometheus's own runtime,** `Prometheus-AGS/universal-agent-runtime`. It ships as a Rust sidecar (`uar-sidecar`) with a loopback HTTP API and AG-UI event streams. The public web has nothing by that name. The integration pattern below comes from The Boss.

---

## 1. Apple's on-device model (default first on Mac)

| | |
|---|---|
| What | Apple's Foundation Models framework (Swift `SystemLanguageModel`). Availability depends on device and region; reported as `.available` / `.unavailable(.deviceNotEligible / .modelNotReady / …)` ([Apple](https://developer.apple.com/documentation/foundationmodels/generating-content-and-performing-tasks-with-foundation-models)). |
| Context | **4,096 tokens per session** ([TN3193](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)); `contextSize` / `tokenCount(for:)` are back-deployed to macOS 26.4. apfel's README says macOS 27 on M3+ with ≥12 GB uses an 8,192-token model (not confirmed by Apple). |
| From Rust | MailCraft already integrates **apfel** (MIT, v1.18.0, 2026-11-15; [github.com/Arthur-Ficial/apfel](https://github.com/Arthur-Ficial/apfel)): `brew install apfel`; `apfel --serve` → OpenAI-compatible server at `http://localhost:11434/v1` (`/v1/chat/completions` with SSE, `/v1/models` → `apple-foundationmodel`, `/health` with the context window). Its identity check is already in `crates/ai/src/on_device.rs`. |
| Tools | None, as long as `--mcp` is never passed. |
| Auth | None (optional `--token` bearer). |
| Verify | apfel's site says macOS 27 ships an Apple CLI called `fm`; unconfirmed in Apple docs. If it exists, it is a second, first-party way in. |
| MailCraft today | Backup provider "Apple's on-device model", confirmed once by the owner, bound to an address + identity (`apps/mailcraft/src/ai/settings.rs`). The new design makes it **first in the chain by default on a Mac** where it is available (still confirmed once, still loopback-only). |

## 2. OpenAI-compatible providers (local and remote)

- **Protocol:** `POST /v1/chat/completions` with `stream: true` returns SSE `data:` chunks ending in `[DONE]`; `GET /v1/models` lists models. **Never send `tools`.** That is the strongest no-tool guarantee available.
- **Common local servers:**
  - Ollama: `http://localhost:11434/v1`; ignores the key; `tool_choice` unsupported ([docs](https://docs.ollama.com/api/openai-compatibility)).
  - LM Studio: default `http://localhost:1234/v1`; optional bearer token ([docs](https://lmstudio.ai/docs/developer)).
  - llama.cpp `llama-server`: the master README now says port **9931** (older docs say 8080); `--api-key`; `/health`.
- **Port clash:** apfel and Ollama both use 11434. `/v1/models` returning only `apple-foundationmodel` identifies apfel (already implemented).
- **MailCraft today:** this is the existing provider model (`ProviderKind::{Local, Remote, Gateway}` in `crates/ai/src/config.rs`): local = loopback plain HTTP checked on the connected address; remote = HTTPS + per-account consent; gateway = loopback that forwards to the internet, so it needs per-account consent. **Keep it as is**; this runtime is that model.

## 3. ChatGPT plan via Sign in with ChatGPT (new)

- **What:** OpenAI OAuth for third-party apps. Scope `chatgpt.tokens.use.direct`; requests go to the Responses API. A separate set of "Sign in with ChatGPT Terms" applies ([Service Terms](https://openai.com/policies/service-terms/)).
- **Eligibility:** open-source projects and locally run personal projects. Commercial private apps are by trial ([quickstart](https://developers.openai.com/siwc/quickstart)). **Action:** register MailCraft as an OAuth client with OpenAI. That gives MailCraft its own client id; it never borrows another program's.
- **Request constraints (unconfirmed, secondary source [WorkOS](https://workos.com/blog/sign-in-with-chatgpt-plan-usage-scope)):** `store:false` and `stream:true` required; system-role items rejected (put instructions in the developer/first user item); no `temperature` or `max_output_tokens`; 1-hour access tokens and 30-day refresh tokens. **Verify against OpenAI's SIWC reference at assess.**
- **Flow:** standard OAuth 2.0 authorization code with PKCE (S256), a loopback redirect bound **before** the browser opens, and a checked `state`. The Boss's generic runtime does exactly this (`the-boss:src/main/services/oauth/runtime/README.md`, `PkceOAuthClient`, `LoopbackCallbackTransport`). Copy the pattern, not its Codex identity.
- **Tokens:** only the refresh token (and account id) goes in the OS credential store (MailCraft's `Services::secrets`). The access token stays in memory, because the blocking constraint says access tokens are never stored.
  - Refresh happens when the token is within 60 s of expiry, and once after an HTTP 401.
  - Only one refresh runs at a time.
  - The write is compare-and-set on the refresh token, so a sign-out that happens mid-refresh isn't overwritten (pattern from `the-boss:OAuthRuntimeService.ts:24,129,409-496`, `OAuthTokenStore.ts:27-62`).
  - A 4xx from the token endpoint is terminal: the sign-in is cleared and the runtime turns off and says so.
- **Tools:** none. MailCraft never sends a `tools` array.

## 4. Claude Code (CLI)

- **Detect.** Latest is v2.1.326 (2026-11-16).
  - Install sources: native installer (`~/.local/bin/claude` → `~/.local/share/claude/versions/`; `%USERPROFILE%\.local\bin` on Windows), `brew install --cask claude-code`, `winget install Anthropic.ClaudeCode`, or `npm i -g @anthropic-ai/claude-code` ([setup](https://code.claude.com/docs/en/setup)).
  - Version: `claude --version`. Diagnostics: `claude doctor`.
  - Native installs auto-update.
- **Run (headless)** ([cli-reference](https://code.claude.com/docs/en/cli-reference), [headless](https://code.claude.com/docs/en/headless)):
  ```
  claude -p --output-format stream-json --verbose --include-partial-messages \
         --tools "" --disallowedTools "mcp__*" --strict-mcp-config \
         --setting-sources "" --no-session-persistence --max-turns 1 \
         --system-prompt <MailCraft's instructions> --model <model>
  ```
  The prompt goes on **stdin** (never on argv, which other processes can read).
  - **Events:** `system` (`init`, `api_retry`), `assistant`, `stream_event` (partial deltas), and a final `result` carrying `result`, `is_error`, `usage`, `total_cost_usd` and `session_id`. Result subtypes: `success`, `error_max_turns`, `error_during_execution`.
  - **Caveat:** `--bare` *adds* Bash, Read and Edit tools, so never use it without `--tools ""`. `--bare` also never reads OAuth or the keychain.
  - **Verify at assess:** `--setting-sources ""` accepts an empty list. If not, run with `CLAUDE_CONFIG_DIR` set to a MailCraft-owned empty folder; The Boss isolates its config directory the same way (`the-boss:…/claudeCode/environment.ts:159-193`).
- **Auth** ([authentication](https://code.claude.com/docs/en/authentication)):
  - Precedence: cloud-provider env vars > `ANTHROPIC_AUTH_TOKEN` > **`ANTHROPIC_API_KEY`** > `apiKeyHelper` > `CLAUDE_CODE_OAUTH_TOKEN` > profile credentials > `/login` subscription. In `-p` mode `ANTHROPIC_API_KEY` always wins when present.
  - **Status:** `claude auth status` prints JSON (exit 0 = logged in, 1 = not), with `authMethod` ∈ {`none`, `claude.ai`, `oauth_token`, `api_key`, `api_key_helper`, `third_party`}.
  - **Storage:** macOS Keychain, falling back to `~/.claude/.credentials.json` (0600); the same file on Linux and Windows.
  - Login: `claude auth login`, or `claude auth login --console` for API billing.
- **MailCraft policy:**
  - **(a) API key** stored in MailCraft's keychain entry and passed only as `ANTHROPIC_API_KEY` in the child's environment for each request. MailCraft clears every other `ANTHROPIC_*` / `CLAUDE_CODE_OAUTH_TOKEN` variable it inherited.
  - **(b) Claude Code's own sign-in only if `claude auth status` says `api_key` / `api_key_helper` / `third_party`** (Console, Bedrock or Vertex billing).
  - `claude.ai` and `oauth_token` (subscription or `setup-token`) are **declined**, with the message shown in the prototype.
  - MailCraft never reads the credential itself. The Boss's presence-only keychain probe (`security find-generic-password -s "Claude Code-credentials"` without `-w`) is unnecessary because `claude auth status` exists.
- **Alternative with no CLI at all:** call Anthropic's Messages API directly with the API key, as a remote OpenAI-compatible-style provider. It's simpler and has no tool surface. Offer the CLI path only because the owner asked for Claude Code detection; it adds no capability for text answers.

## 5. Codex (CLI)

- **Detect.** Latest is `rust-v0.169.0` (2026-11-13).
  - Install: `curl … chatgpt.com/codex/install.sh | sh`, `npm i -g @openai/codex`, `brew install --cask codex`, or release binaries ([repo](https://github.com/openai/codex)).
  - Version: `codex --version`.
  - State lives in `CODEX_HOME` (default `~/.codex`).
  - **Verify:** the standalone installer's binary location.
- **Run** ([non-interactive](https://learn.chatgpt.com/docs/non-interactive-mode)):
  ```
  codex exec --json --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules \
             --sandbox read-only -a never -  (prompt on stdin)
  ```
  Run it with `cwd` = a fresh empty temp directory, and `CODEX_HOME` = the owner's (for auth) **or** a MailCraft-owned one (decide at assess; see G-RT6).
  - **Events (JSONL):** `thread.started`, `turn.started`, `turn.completed` (with `usage`), `turn.failed`, `item.started` / `item.completed` (`agent_message`, `reasoning`, `command_execution`, file changes, MCP calls, web searches), `error`.
  - **MailCraft treats any `command_execution`, file-change or MCP item as a policy violation:** it kills the process, discards the answer and logs the event kind (no content).
- **Containment caveat:** no documented flag removes the shell tool. The app-server's `readOnly` policy defaults to full-disk read access ([app-server](https://learn.chatgpt.com/docs/app-server)). Hence: **off by default**, a warning in Settings, the empty-cwd + ephemeral + ignore-user-config recipe, and kill-on-tool-event.
- **Auth** ([auth](https://learn.chatgpt.com/docs/auth)):
  - Login: `codex login` (browser, localhost:1455), `codex login --device-auth` (beta), `printenv OPENAI_API_KEY | codex login --with-api-key`, or the per-run env var `CODEX_API_KEY`.
  - Status: `codex login status`.
  - Storage: `cli_auth_credentials_store = file | keyring | auto | ephemeral`; the file is plaintext `$CODEX_HOME/auth.json`.
  - **MailCraft policy (G-RT12):** use Codex with an **API key only**, passed as `CODEX_API_KEY` for each run from MailCraft's keychain entry. Each run gets fresh `CODEX_HOME` and `HOME` folders, so neither the owner's `auth.json` nor Codex's own login (ChatGPT or API key) is ever read or used. OpenAI's sanctioned route for ChatGPT plans is SIWC (§3), and its docs steer automation to API keys.
- **SDKs:** the TypeScript `@openai/codex-sdk` spawns `codex exec --experimental-json`. The Python SDK drives `codex app-server` (JSON-RPC over stdio). `codex mcp-server` has been removed ([codex-sdk](https://learn.chatgpt.com/docs/codex-sdk)). For Rust, `codex exec --json` is the stable surface. app-server is richer: approvals as server-to-client requests, which MailCraft would always answer "deny".

## 6. OpenCode

- **Releases:** V1 `v1.19.6` (npm `opencode-ai`), V2 `v2.3.0` (npm `@opencode/cli`). Docs describe V2 ([commands](https://opencode.ai/docs/cli/commands)). **Verify** install locations and the `--version` flag; use `opencode debug paths`.
- **Run:**
  - Headless: `opencode run --format json --model provider/model --standalone "<msg>"` (NDJSON; **verify** the event schema).
  - Server: `opencode serve` (127.0.0.1:4096, `OPENCODE_SERVER_PASSWORD` basic auth, OpenAPI spec).
  - ACP: `opencode acp`.
- **Permissions** ([permissions](https://opencode.ai/docs/permissions.md)): **default allow-all**. MailCraft runs it with a MailCraft-owned config containing `{ "action": "*", "resource": "*", "effect": "deny" }` and a policy that overrides project config ([policies](https://opencode.ai/docs/policies)), plus an empty cwd.
- **Auth** ([providers](https://opencode.ai/docs/cli/providers.md)):
  - Commands: `opencode auth login|list --format json|logout`.
  - V2 stores credentials in SQLite (`~/.local/share/opencode/opencode.db`).
  - OpenCode is a **SIWC launch partner**, so a ChatGPT plan through OpenCode is sanctioned by OpenAI. Its Claude subscription sign-in was removed.
  - MailCraft reads only `opencode auth list --format json` (provider names and methods, never tokens) to show status.
- **The Boss:** launches it only in a terminal and writes `opencode.json` with `cherry-*` providers (`the-boss:…/cliConfig/builders.ts:200-240`); it has no own-login support for it.

## 7. Universal Agent Runtime (UAR)

All from The Boss; there is no public spec.

- **What:** `Prometheus-AGS/universal-agent-runtime`, a Rust runtime shipped as `uar-sidecar[.exe]` per platform, with a manifest-checked payload (`the-boss:uarPayload.ts:37-92`).
- **Discovery:** `GET http://127.0.0.1:1906/.well-known/uar-config` or `/.well-known/agent-card.json` (`name: "UAR Compiler Agent"`), with a 2 s timeout and 1 MB cap. This is a hint only (`uarStartup.ts:33-49`). If an instance already runs on 1906 and the setting is "ask", the host refuses to start a second one.
- **Lifecycle** (`UarSidecarService.ts`):
  1. Spawn `uar-sidecar --config <file> --port <n>`.
  2. Write a random 32-byte **launch token on stdin**.
  3. Wait for `READY:<port>` on stdout.
  4. All requests go to `http://127.0.0.1:<port>` with `Authorization: Bearer <launchToken>`.
  5. Lifecycle control arrives as `LIFECYCLE:{json}` lines on stdout.
  6. Closing stdin shuts it down.
  7. A generation counter rejects requests that cross a restart.

  The environment is **sanitized**: `UAR_*`, `LLM_*` and provider-key variables are removed. **Native file, web and terminal tools are disabled.** The data directory is 0700 and secrets files are 0600.
- **Protocol:** `POST /api/uar/runs` (and `/runs/<id>/resume`, `/cancel`); events from the returned `stream_url?stream_mode=agui_spec` as an **AG-UI** stream with `last-event-id` resume. Agents CRUD at `/api/agents`. UI profile `uar.a2ui/1`.
- **Capabilities:** UAR accepts models over anthropic-messages, openai-chat/responses and ollama-chat (`agentRuntimeCapabilities.ts:100-215`).
- **Fit for MailCraft:**
  - A UAR client is an HTTP + SSE consumer, which MailCraft's `crates/net` already does (streaming POST, SSE parser, loopback enforcement).
  - MailCraft would consume only text-message events and treat any tool-call event as a violation.
  - The phase goals already mention AG-UI.
- **Where UAR sends mail:** depends on which model it routes to. MailCraft must treat it as **"leaves this computer" unless UAR reports a local model**, which needs a UAR capability query (**verify** in UAR's API at assess).
- **Not settled:** whether MailCraft bundles `uar-sidecar` (an asset/licence question under AGENTS.md's attribution rules) or only detects and starts an installed one. The plan recommends detect-and-start only (G-RT7).

## 8. A universal layer? ACP

- The **Agent Client Protocol** (Zed) is JSON-RPC over stdio. Rust crate `agent-client-protocol` 5.3.0 ([libraries/rust](https://agentclientprotocol.com/libraries/rust)).
- Agents: Claude via the `claude-agent-acp` adapter, Codex via `codex-acp`, and native support in OpenCode, Gemini CLI, Goose, Kimi and others ([agents](https://agentclientprotocol.com/get-started/agents)).
- **Not recommended as the primary path:**
  - It's built for coding sessions (cwd, tool calls, permission requests).
  - The Claude and Codex adapters are Node programs.
  - It changes nothing about subscription policy.
- Keep it as a **future adapter** behind the same runtime trait if more agents need to be supported.

## 9. Detection patterns worth porting from The Boss

- **One preset table** of CLIs: id, executable, install sources (`the-boss:src/shared/data/presets/codeCliTools.ts:44-154`).
- **Detection order:**
  1. App-managed copy.
  2. The **login-shell PATH**: run `$SHELL -lc env` with a 15 s timeout, because GUI apps on macOS don't inherit the shell PATH.
  3. Resolve the name with `command -v "$1"`, passed as a positional argument, never interpolated (`the-boss:src/main/utils/shellEnv.ts:145-180`, `commandResolver.ts:205`).
  4. Then MailCraft adds `--version` (The Boss doesn't, and shows no version for system installs).
- **Status vocabulary:** not installed / installing / installed (version) / needs repair, with failures kept as a persistent row rather than a toast (`VersionStatusCard.tsx`).
- **Config ownership:** if MailCraft ever writes a CLI's config, it owns only keys with a prefix of its own, writes atomically at 0600, and rolls back on failure (`the-boss:configWriter.ts:74-117`). MailCraft's design avoids writing CLI config at all, using env vars and its own config directories instead, which is safer.
- **Never build shell strings.** The Boss uses AppleScript and `.bat` files to open terminals. MailCraft spawns argument arrays directly.

## 10. Things not to port

| The Boss pattern | Why not |
|---|---|
| Codex OAuth with the Codex CLI's client id, plus `chatgpt.com/backend-api/codex` | Impersonates another program; breaks `no-foreign-oauth-client-ids`; not the sanctioned SIWC route |
| Claude Code on a Pro/Max login (`external-cli` route) | Anthropic's terms forbid third-party routing through subscription credentials |
| Copilot and Grok client ids, `copilot_internal` endpoint | Same impersonation problem |
| OAuth tokens unencrypted in SQLite (`ProviderService.ts:742-749`) | MailCraft keeps secrets only in the OS credential store |
| API keys written into CLI config files | Pass them per request through the child's environment instead |
| Shared secrets in the renderer bundle | No shared secrets in MailCraft binaries |

## 11. Open verification list (for assess)

1. SIWC: official request constraints, token lifetimes, client registration steps and terms for an open-source desktop app.
2. `claude --setting-sources ""` behaviour; `claude auth status` JSON field names on the installed version.
3. Codex: standalone install path; whether a future flag removes the shell tool; `CODEX_API_KEY` precedence over `auth.json`.
4. OpenCode V2: install paths, `--version`, the `run --format json` event schema, the deny-all config location (env var for config path).
5. UAR: an API to report whether the routed model is local; licence and distribution terms for `uar-sidecar`.
6. Apple: the `fm` CLI on macOS 27 and the 8,192-token context.
