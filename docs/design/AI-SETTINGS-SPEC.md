# AI assistance settings: agent runtimes, configuration and sign-in

**Status:** design spec for the `agent-runtimes` phase (see `PLAN.md` Part B). Nothing here is decided until the owner answers the gates in PLAN.md §B3.
**Companion files:**
- `settings.html` — clickable prototype of File › Options › AI assistance.
- `research/AGENT-RUNTIMES.md` — sources, versions and the vendor-policy findings this spec relies on.
**Builds on:** what the AI phase shipped:
- `crates/ai` — `ProviderConfig`, `ProviderKind::{Local, Remote, Gateway}`, `on_device`;
- `apps/mailcraft/src/ai/{settings,route,keys,device,run}.rs` — `ai.json` v2: primary, backup, consents, `keys_kept_for`;
- `crates/ui-egui/src/ai/settings_*.rs` — the "AI assistance" page.

Line references are as of 2026-11-17; re-read before editing.

---

## 1. Goals and non-goals

**Goals**

- Seven runtime types behind one settings page and one request path:
  1. Apple's on-device model (default first on Mac);
  2. OpenAI-compatible providers (what exists today);
  3. ChatGPT plan via Sign in with ChatGPT;
  4. Claude Code;
  5. Codex CLI;
  6. OpenCode;
  7. Universal Agent Runtime (UAR).
- An **ordered chain** of runtimes, replacing primary + backup, with a stated fallback policy.
- **Detection** of installed runtimes, with a version and a plain status.
- **Sign-in automation:**
  - MailCraft runs the OAuth flow itself for ChatGPT plan.
  - MailCraft starts the runtime's own sign-in and watches for the result for the CLIs.
  - API keys are stored once in the OS credential store and handed to a runtime per request.
- **Token management:**
  - refresh, expiry and revocation for tokens MailCraft owns;
  - status-only for tokens a CLI owns, which MailCraft never reads.
- Every existing rule still holds: local-first, per-account consent for anything that leaves the computer, mail is data never instructions, no secrets in files or logs.

**Non-goals**
- Tool use by any runtime on mail. Every runtime is used as a **text model**.
- Installing or updating CLIs. MailCraft detects; the owner installs. This differs from The Boss, which downloads binaries.
- Using a Claude Pro/Max subscription. Anthropic's terms forbid it (research §0.1).
- Reusing another program's OAuth client identity (constraint `no-foreign-oauth-client-ids`).
- Runtimes in the browser (wasm) build. It keeps OpenAI-compatible HTTP only.

---

## 2. Concepts

| Term | Meaning |
|---|---|
| **Runtime** | One way of getting an answer: `on_device`, `compat:<provider-id>`, `chatgpt`, `claude_code`, `codex`, `opencode`, `uar`. |
| **Where** | `local` (text never leaves the computer) or `leaves` (it goes to a vendor or service). **Fixed per runtime type, never self-reported:** `on_device` = `local`; `compat` = from the provider kind (Local → `local`; Remote and Gateway → `leaves`); `chatgpt`, `claude_code`, `codex`, `opencode` = **always `leaves`** (each sends text to a vendor model under the owner's account); `uar` = `leaves`, except a **managed** instance whose only model endpoint MailCraft itself configured as a verified loopback `compat:Local` provider (§7.6). A runtime's own claim never makes it `local`. |
| **Chain** | The owner's ordered list of runtimes; each is on or off. |
| **Consent** | Per account × per runtime yes, required when *where* = `leaves`. This is the existing `set_consent`, generalised from provider ids to runtime ids. |
| **Credential owner** | `mailcraft`: MailCraft holds the secret in the OS credential store (API keys, SIWC tokens, the UAR launch token in memory). `runtime`: the CLI holds it and MailCraft only asks for status. |
| **Containment recipe** | The exact flags, environment, working directory and kill rules that make a runtime text-only (§7). |

---

## 3. Settings file: `ai.json` version 3

The file stays hostile input:
- size cap 256 KB;
- unknown runtime kinds dropped (never taken as local);
- every field checked;
- nothing logged;
- never a secret.

```jsonc
{
  "version": 3,
  "never_configured": false,
  "fallback": "next",                 // "next" | "ask" | "stop"
  "chain": [                          // order = order tried; at most 16 entries
    { "runtime": "on_device", "enabled": true },
    { "runtime": "chatgpt",   "enabled": true,  "model": "default" },
    { "runtime": "compat:ollama", "enabled": true },
    { "runtime": "claude_code",   "enabled": false, "model": "sonnet",
      "path": null,                   // owner override of the binary path, or null = detected
      "auth": "api_key" },            // "api_key" | "own_api_billing"
    { "runtime": "codex",     "enabled": false, "auth": "api_key", "path": null },
    { "runtime": "opencode",  "enabled": false, "model": "openai/gpt-5", "path": null },
    { "runtime": "uar",       "enabled": false, "instance": "managed" }   // "managed" | "external"
  ],
  "on_device": { "address": "http://127.0.0.1:11434", "identity": "…", "context_window": 4096 },
  "on_device_ports": [11434],
  "providers": [ /* unchanged ProviderConfig list for compat:* */ ],
  "consents": {                       // account address -> runtime ids allowed to receive its mail
    "avery@lumen.example": ["chatgpt"]
  },
  "keys_kept_for": ["compat:openrouter", "claude_code", "codex", "chatgpt"],
  "detected": {                       // cache only; re-derived on rescan; never trusted for decisions
    "claude_code": { "path": "/Users/…/.local/bin/claude", "version": "2.1.326", "checked": "2026-11-17T10:00:00Z" }
  }
}
```

### 3.1 Migration from v2

| v2 | v3 |
|---|---|
| `primary: "<id>"` | `chain[0] = compat:<id>`, enabled |
| `backup: "apple-on-device"` + confirmed `on_device` binding | `chain[1] = on_device`, enabled; **on macOS, if the binding is confirmed, `on_device` moves to `chain[0]`** only if the owner accepts the one-time question (G-RT1) |
| primary explicitly None, **no** compat backup | chain with every entry disabled; `never_configured: false` |
| primary explicitly None **and** a compat backup | the backup becomes an enabled `compat:<id>` entry, first; every other entry is disabled. **Precedence: this row wins over the row above** |
| primary set, `backup` absent / null / `""` | primary as `chain[0]`; no backup entry |
| primary set **and** a compat backup | primary as `chain[0]`, backup as `chain[1]`, both enabled |
| credential entries `ai.<provider-id>` (v2 key names) | copied to `ai.compat:<provider-id>`; the old entry is deleted **only after** the new one is written and read back; `keys_kept_for` lists both names until the old one is gone |
| `never_configured: true` | v3 `never_configured: true`; first-run behaviour per §8 |
| provider consents | `consents[account] += "compat:<id>"` |

- The migrated file is written once, and the v2 file is kept as `ai.v2.json` for one release, so a rollback still works.
- Tests cover every row of the table plus a malformed v2.

### 3.2 Where each thing lives

| Data | Where | Why |
|---|---|---|
| Chain, models, paths, fallback, consents | `ai.json` | Owner settings |
| API keys (compat remote, Claude Code, Codex) | OS credential store via `Services::secrets`, service `mailcraft`, entry `ai.<runtime-id>` | Constraint `secrets-never-persisted-or-logged` |
| SIWC **refresh token**, expiry, account id | One credential-store entry `ai.chatgpt` holding `{refresh_token, account_id}`. The **access token lives in memory only** and is re-derived from the refresh token after a restart (blocking constraint `secrets-never-persisted-or-logged`: "access tokens never stored") |
| UAR launch token | Memory only, new for each spawn | It's a session secret |
| CLI-owned credentials (Claude Code keychain, Codex `auth.json`, OpenCode SQLite) | **Never read by MailCraft** | Status comes from the CLI's own status command |
| Detection cache | `ai.json` `detected` | Speeds up the Options page; re-derived on Look Again |

---

## 4. Layering (AGENTS.md architecture)

| Layer | Owns | New modules (proposed) |
|---|---|---|
| `crates/model` (pure) | runtime ids, `Where`, chain types, consent checks | `model::ai::runtime` |
| `crates/ai` (L5, transport, no egui) | parsing CLI status JSON and event streams; SIWC token types and refresh decisions; containment arg/env builders; UAR wire types | `ai::runtime::{claude_code, codex, opencode, uar, siwc}` — pure builders and parsers; **each adapter change (B5.5–B5.9) adds the fuzz target for its own stream parser**; B5.4 provides the shared fuzz harness and the tool-event detector interface |
| `apps/mailcraft` (host) | spawning processes; login-shell PATH; OAuth loopback listener; credential store; timers | `apps/mailcraft/src/ai/runtime/{detect, spawn, siwc, uar_sidecar}.rs` |
| `crates/engine` | `ai.*` command specs and agent classes (**engine-engineer only**); no I/O | new owner-only commands (§10) |
| `crates/ui-egui` | settings page, sign-in dialogs, status | `ai/settings_runtimes.rs`, `ai/signin.rs` |

- The engine never spawns or connects. That's the existing request/apply pattern: the engine asks, the host runs `Host::run_job`.
- **wasm:** the runtime modules are `cfg(not(target_arch = "wasm32"))` in the host. The model and parsers stay portable, so wasm keeps building.

---

## 5. Detection

### 5.1 Algorithm (host, on Options open, Look Again, and lazily before first use)
1. **Get the login-shell PATH once per session.**
   - **macOS / Linux:** spawn `$SHELL -lc 'printf %s "$PATH"'`.
     - `$SHELL` must be an absolute path to an existing file in `/etc/shells`; otherwise use `/bin/sh`.
     - 10 s timeout; 64 KB cap on the output.
   - **Windows:** the registry PATH.

   GUI apps on macOS don't inherit the shell PATH (pattern from `the-boss:shellEnv.ts`).
2. **For each runtime, check candidates in order**:
   1. the owner's path override;
   2. the login-shell PATH, resolved in Rust by walking PATH entries (no `sh -c`);
   3. well-known locations:

| Runtime | Executable | Well-known locations |
|---|---|---|
| Claude Code | `claude` | `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `%USERPROFILE%\.local\bin` |
| Codex | `codex` | `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, npm global bin |
| OpenCode | `opencode` | `~/.opencode/bin`, `/opt/homebrew/bin`, npm global bin |
| UAR | `uar-sidecar` | `~/.prometheus/bin`, plus probe `127.0.0.1:1906/.well-known/uar-config` (2 s, 1 MB cap) |
| apfel | `apfel` + its server | existing on-device discovery (`device.rs`) |

3. **Version probe:** `<exe> --version` with no shell, an empty cwd, `stdin=null`, a 5 s timeout and a 4 KB output cap. Parse the first semver-looking token; keep the raw string ≤ 64 chars. Failure gives `Broken`, not `Missing`.
4. **Auth status probe** (§6.3), only for runtimes that are found.
5. **Results:** `Detected { path, version, source: override|path|wellknown, checked_at }`, written to the cache.

### 5.2 Rules
- Detection **never** runs a runtime with a prompt, opens the network (except UAR's loopback probe), or reads credential files.
- Paths are shown in full in the detail panel. The executable must be a regular file. On Unix, MailCraft warns (and doesn't run it) if it is group- or world-writable.
- Detection runs at most once a minute unless the owner chooses Look Again.
- **The same executable checks run again at every spawn**, for detected paths and for owner overrides (`ai.json` is hostile input): an absolute path; a regular file after resolving symlinks; owned by the owner or root; not group- or world-writable; the same for its parent directory. A failure gives `Broken` with the reason, and nothing runs. Covered by SCN-RT-PATH-001.

---

## 6. Sign-in and token management

### 6.1 Who owns which credential

| Runtime | Credential | Owner | How MailCraft gets status | How it signs in |
|---|---|---|---|---|
| on_device | none | — | `/v1/models` + `/health` identity (existing) | One-time confirm (existing) |
| compat (remote / gateway) | API key | MailCraft | key present in credential store; Test Connection | Key dialog (existing) |
| chatgpt | SIWC tokens | MailCraft | token present + not terminally refused | **MailCraft's own OAuth flow** (§6.2) |
| claude_code | API key **or** Claude Code's API-billing sign-in | MailCraft **or** runtime | `claude auth status` (JSON) | Key dialog, **or** start `claude auth login --console` (§6.4) |
| codex | API key | MailCraft | `CODEX_API_KEY` present in MailCraft's credential store (MailCraft never runs `codex login`) | Key dialog only. Codex's own sign-in is **not used**: MailCraft runs Codex with an isolated, per-run `CODEX_HOME` (§7.3), so neither the owner's ChatGPT login nor an API-key login in `~/.codex` is reachable. This narrows the owner's request ("automated OAuth for Codex"); it is recorded as gate G-RT12 |
| opencode | provider sign-ins in OpenCode | runtime | `opencode auth list --format json` (names and methods only) | Start `opencode auth login <provider>` in a terminal, or tell the owner the command |
| uar | per-spawn launch token; UAR's own model credentials | MailCraft (launch token) / runtime (model keys) | `READY:<port>` handshake + capability check | Start the sidecar (§7.6) |

### 6.2 ChatGPT plan: Sign in with ChatGPT (MailCraft-owned OAuth)

**Prerequisite:** MailCraft is registered with OpenAI as a SIWC client (open-source eligibility, research §3). **The client id is a build-time constant** read with `option_env!("MAILCRAFT_SIWC_CLIENT_ID")` by the release build. It is never read from `ai.json` or any runtime input, so a hostile settings file can't swap in another client. When the build has no id, the ChatGPT plan runtime is hidden ("Not available in this build"); that is the G-RT3 escape. It is MailCraft's own id, which satisfies `no-foreign-oauth-client-ids`. Public clients use PKCE and no client secret.

**Flow (host):**
1. Generate a PKCE verifier (32 random bytes, base64url) and S256 challenge, a random `state` (32 bytes), and a nonce if OIDC applies.
2. **Bind the loopback listener first:** `127.0.0.1:0` (ephemeral) if OpenAI allows any port for registered loopback redirects. Otherwise use the registered fixed port; if it is busy, fail with "Another program is using port N".
3. Open the authorization URL in the default browser via `Services::open_url`, never a webview.
4. **Callback handling.** Accept exactly one request to the redirect path. Verify `state` in constant time. Reply with a static page that has no external resources ("You can close this tab"), then close the listener. Wait at most 10 minutes; Cancel closes the listener.
5. **Token exchange** over HTTPS (`crates/net`, TLS-only), with response size caps. Store `{refresh_token, account_id}` in the credential entry `ai.chatgpt`; keep the access token and its expiry **in memory only**. After a restart the first request refreshes first.
6. Show "Signed in as ‹email› (‹plan›)". The email and plan come from the ID token or account endpoint, shown as untrusted text: capped and stripped of control characters.

**Refresh:**
- When `expires_at − now < 60 s`, or once after an HTTP 401: hold a per-runtime mutex, re-read the entry, and refresh only if the stored refresh token is still the one that triggered it (compare-and-set).
- Write the new refresh token atomically. **Terminal** only for OAuth grant errors that mean the sign-in is gone: `invalid_grant`, `invalid_client`, `unauthorized_client`, or an explicit revocation response. Then delete the entry, set status `SignInEnded`, turn the runtime off and tell the owner why ("OpenAI ended this sign-in"). **Transient**, with exponential backoff and the stored entry kept: 408, 429 (honouring `Retry-After`), other 4xx without a terminal grant error, 5xx and network errors. The request fails over per the chain policy.

**Sign out:**
- Delete the entry, call the revocation endpoint if OpenAI documents one (best effort), and remove `chatgpt` from every account's consents.
- The UI says what happened and that the owner can also remove MailCraft in their ChatGPT settings.

**Requests:**
- Responses API with `stream: true`, `store: false` and **no `tools`**. The access token is taken from memory; if absent or expiring, refresh first (§6.2).
- MailCraft's instructions go in the first developer/user item if system items are rejected (verify, research §11.1).
- Mail text goes in a clearly delimited data block, as the existing structured prompt does.

### 6.3 Status probes for CLI-owned sign-ins (never read secrets)

| Runtime | Command | Parse | Allowed states |
|---|---|---|---|
| Claude Code | `claude auth status` (exit 0/1, JSON) | `authMethod` plus the field that says how the account is billed (**verify at assess** which field distinguishes a Console / API-billed OAuth login from a Claude.ai subscription, e.g. a billing or subscription-type field) | **Usable:** `api_key`, `api_key_helper`, `third_party`, **or** an OAuth login whose billing field says API/Console. **SubscriptionDeclined:** `claude.ai`, `oauth_token`, or any OAuth login whose billing is a Pro/Max/Team/Enterprise subscription **or can't be determined**. **NeedsSignIn:** `none`. **If assess finds no field that tells Console OAuth from subscription OAuth, the "Claude Code's own Console sign-in" option is removed and Claude Code is API-key only** (record in decision-log). |
| Codex | **none**: Codex's own sign-in is never probed or used (G-RT12). Status comes only from whether MailCraft holds an API key for it | — | key present → usable; no key → NeedsKey |
| OpenCode | `opencode auth list --format json` | provider names + methods | at least one provider matching the configured `model` prefix → usable |

- Every probe runs with no shell, a 5 s timeout, a 16 KB cap, the same environment as real runs (§7.1), and an empty cwd.
- The output is hostile: parse with caps, and never show raw output except a capped "Details" fold.

### 6.4 Starting a CLI's own sign-in ("automate")

- **Claude Code (API billing):** MailCraft spawns `claude auth login --console`. No shell, an empty cwd, MailCraft's env allow-list, and a PTY is **not** used: if the CLI requires a TTY, fall back to opening the owner's terminal with the fixed argv.
  - The CLI opens the browser itself.
  - MailCraft polls `claude auth status` every 2 s for up to 10 minutes and shows "Finish signing in in your browser".
  - Success means `authMethod` is in the allow-list. A resulting `claude.ai` method is reported as declined.
- **Codex:** no sign-in is started. MailCraft holds the API key (key dialog) and passes it as `CODEX_API_KEY` per run; see G-RT12.
- **OpenCode:** `opencode auth login <provider>` is interactive. MailCraft opens the owner's terminal with that fixed argv: the provider id is validated against `[a-z0-9-]{1,32}` and passed as one argument (The Boss uses a fixed `/login` boolean for the same reason). Then it polls `auth list`.
- **Never** pass a key on a command line (other processes can read it), and never write a key into a CLI's config files.

### 6.5 Expiry and revocation (all runtimes)

- Before each request, the host checks status cheaply:
  - MailCraft-owned credentials: entry present, not expired.
  - CLI-owned credentials: re-probe at most every 5 minutes, or after a run fails with an auth error.
- An auth failure during a run maps to `ErrorKind::SignedOut`. The chain moves on per policy, and the status chip and detail panel show "Sign-in expired".
- **Removing MailCraft's data** (existing feature) deletes every `ai.*` credential entry named in `keys_kept_for`. It never touches CLI-owned stores.

---

## 7. Invocation contracts (containment recipes)

### 7.1 Common to every spawned runtime

- **Spawn:** argv array, **no shell**. The working directory is a fresh empty temp directory (0700), deleted after the run. `stdin` carries the prompt; `stdout` and `stderr` are read with caps (stdout 4 MB, stderr 64 KB).
- **Safe Rust only** (`unsafe_code = "forbid"`). Use `std::os::unix::process::CommandExt::process_group(0)` (stable and safe) instead of `pre_exec`/`setsid`; `nix::sys::signal::killpg` to kill the group; a safe job-object wrapper on Windows. Sandboxes are applied **without `pre_exec`**: on macOS by launching through `sandbox-exec` (argv); on Linux by a tiny MailCraft launcher mode that restricts *itself* with the safe `landlock` crate and then `exec`s the runtime, or through `bwrap` (argv). Assess must confirm a safe crate for each platform (gate G-RT13) before W2; if one needs `unsafe`, the owner decides against `no-unsafe`.
- **Timeouts:** wall clock as configured (default 120 s), plus 30 s with no output. Cancel kills the **whole process group** (Unix `setsid` + `killpg`; Windows job object).
- **Environment:** built from an **allow-list**, never inherited wholesale.
  - **`HOME` is a fresh per-run directory** inside the run's temp folder for every runtime whose credentials MailCraft supplies (Claude Code with an API key, Codex, UAR). Only Claude Code using its own Console sign-in and OpenCode get the real `HOME`, because their credentials live there. Those two are covered by the OS sandbox below.
  - Allowed: `PATH` (login-shell PATH), `HOME`, `USER`, `LANG`, `TMPDIR`/`TEMP`, plus the runtime's own credential variable when MailCraft owns it.
  - **Removed:** `ANTHROPIC_*`, `OPENAI_*`, `CODEX_*`, `CLAUDE_*`, `OPENCODE_*`, `UAR_*`, `LLM_*`, `*_API_KEY`, `NODE_OPTIONS`, proxies, unless explicitly set by the recipe.
- **Output:** parsed as hostile JSON lines with per-line caps. Only text deltas and the final text are used. **Any tool-use, command, file or MCP event kills the run** and is reported as "The runtime tried to use a tool; MailCraft stopped it". The event *kind* is logged, never its content.
- **Prompt:** MailCraft's existing structured prompt (injection defences, mail as delimited data, citation rules), with the runtime's own system prompt replaced where possible.
- **Argv never carries prompt or mail text.** The prompt goes on stdin. Instructions go in a 0600 file inside the run's 0700 temp folder, passed by path (e.g. `--system-prompt-file`; **verify** the flag at assess), or prepended to stdin when no file flag exists.
- **OS sandbox (preventive, not just detective).** Kill-on-tool-event happens *after* a tool ran, and a read file may already have gone to the vendor. Every spawned runtime therefore runs under an OS sandbox when one is available. It is denied reads outside its install directory, its own config/credential directory and the run's temp folder, and denied writes outside the temp folder. Network is allowed (the runtime must reach its vendor).
  - macOS: `sandbox-exec` with a generated Seatbelt profile (verify it is still usable for this at assess).
  - Linux: Landlock, or bubblewrap when present.
  - Windows: an AppContainer or restricted token (verify).
  - **Where no sandbox is available, Codex and OpenCode refuse to run** (they can execute commands), while Claude Code (`--tools ""` removes its tools before any run) and UAR (native tools off) may still run, with the residual risk named in the threat model.
- **Residual risk to state honestly (threat model, PRIVACY.md):** a runtime that is talked into reading a file it *can* read sends that text to its vendor before MailCraft sees any event. The sandbox limits what it can read. Kill-on-tool-event limits what happens next. Neither makes text exfiltration impossible.

### 7.2 Claude Code
`claude -p --output-format stream-json --verbose --include-partial-messages --tools "" --disallowedTools "mcp__*" --strict-mcp-config --setting-sources "" --no-session-persistence --max-turns 1 --model <m> --system-prompt-file <run-dir>/instructions.txt`
- Environment: `ANTHROPIC_API_KEY` (when MailCraft owns the key). `CLAUDE_CONFIG_DIR` points at a MailCraft-owned empty directory **unless** the owner chose Claude Code's own API-billing sign-in, which needs the real config directory or Keychain. That case is gated by §6.3's allow-list, re-probed before each run.
- Kill on any `tool_use` content block.

### 7.3 Codex
`codex exec --json --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules --sandbox read-only -a never -`
- Environment: `CODEX_API_KEY` from MailCraft's credential store; `CODEX_HOME` and `HOME` both set to **fresh per-run directories** inside the run's temp folder, deleted after the run. Nothing persists; the owner's `~/.codex` is never read. "Remove MailCraft's data" needs no Codex step.
- Kill on `command_execution`, file changes, MCP calls or web searches.
- **Off by default.** Settings shows the tool caveat (research §5).

### 7.4 OpenCode
`opencode serve --hostname 127.0.0.1 --port 0` with a random `OPENCODE_SERVER_PASSWORD`, then the message is sent over its local HTTP API (verify the V2 API at assess). **No argv fallback:** if neither stdin for `opencode run` nor the server API can carry the message, OpenCode is not offered (G-RT5 b).
- Config: a MailCraft-owned config file with a deny-all permissions array and a policy overriding project config (verify the env var that points OpenCode at it).
- Kill on any tool event.

### 7.5 ChatGPT plan, compat, on_device
In-process HTTP through `crates/ai`'s client (existing), never with `tools`.

### 7.6 UAR
**Managed instance:**
1. Spawn `uar-sidecar --config <MailCraft-owned config> --port 0` with the sanitized environment and a config that disables native file, web and terminal tools.
2. Write a 32-byte launch token on stdin.
3. Await `READY:<port>` within 20 s.
4. Check the required capabilities. **`Where` for a managed instance:** MailCraft writes UAR's model configuration itself. It is `local` only if that configuration names exactly one model endpoint: a MailCraft `compat` provider of kind Local, whose resolved peer MailCraft verified as loopback (operator decision D7). Any other configuration, or any model the instance reports beyond it, makes it `leaves`. UAR's own reports are never trusted for this.
5. Requests go to `127.0.0.1:<port>` with `Authorization: Bearer <token>`; runs via `POST /api/uar/runs`, events from the AG-UI stream. Only text-message events are used; tool-call events cancel the run.
6. Shutdown by closing stdin; a generation counter rejects stale requests.

**External instance (already running on 1906):** used only if the owner chooses it. MailCraft can't contain an external instance's tools, so Settings says so and it counts as `leaves`.

---

## 8. Defaults and first run

| Platform | Default chain | Notes |
|---|---|---|
| macOS with Apple Intelligence available | `on_device` (enabled after a **one-time yes**), then everything else disabled | The yes is local-only and sends nothing; the owner can say no (G-RT1) |
| macOS without it, Linux, Windows | empty / all disabled; `never_configured: true` | Existing behaviour: Find by Description explains and links to Options |
| Browser | compat only | Unchanged |

- Detection may run at first launch on a Mac **only to find apfel** (existing behaviour). Other runtimes are looked for when the owner opens AI assistance.
- No runtime that leaves the computer is ever enabled without the owner, and none is used for an account without that account's consent.

---

## 9. Chain and fallback semantics

1. A request names the accounts whose mail it contains.
2. Walk the chain in order, skipping:
   - disabled entries;
   - entries whose status isn't usable;
   - entries where *where* = `leaves` and **any** account in the request lacks consent for that runtime.
3. On a failure (timeout, auth, policy kill, refused):
   - `next` → try the next eligible entry;
   - `ask` → show "‹A› couldn't answer (‹reason›). Try ‹B›?";
   - `stop` → show the error.
   A **policy kill** (tool use) never falls through silently: the answer pane says it happened, even with `next`.
4. The answer's provenance names the runtime that answered and every one that failed before it. This extends task 6.6's "Answered by" and fallback notice.
5. Agents (MCP) can't trigger this path. The `ai.*` commands stay `Never` for agents, as today.

---

## 10. Commands (owner-only; engine-engineer classifies all as `Never` for agents)

| Id | Params | Effect |
|---|---|---|
| `ui.aiSettings` | — | existing; opens the page |
| `ai.runtimes` | — | list: id, name, where, enabled, status, version, path (no secrets) |
| `ai.setRuntimeOrder` | `{order: [runtime-id]}` | reorder (must be a permutation) |
| `ai.setRuntimeEnabled` | `{runtime, value}` | on/off; turning on a `leaves` runtime doesn't grant consent |
| `ai.setRuntimeOption` | `{runtime, model? , path?, auth?}` | validated per runtime |
| `ai.rescan` | — | detection now |
| `ai.signIn` | `{runtime}` | starts §6.2 / §6.4; answers `{pending: "signIn"}` |
| `ai.signOut` | `{runtime}` | §6.2 sign-out, or forget the MailCraft-held key; never touches CLI stores |
| `ai.setRuntimeKey` | `{runtime}` | opens the key dialog; keys are never accepted as a parameter (control channel or MCP) |
| `ai.setConsent` | `{runtime, account, allowed}` | existing, widened from provider id to runtime id |
| `ai.setFallback` | `{value: next|ask|stop}` | |

- All are added to `hostile_params_never_panic`.
- Control-channel exposure for owner scripts follows the existing `control/ai.rs` pattern (refused to agents at `agent_gate.rs:99`).

---

## 11. UI (maps to `settings.html`)

- **Summary chips:** first runtime that can answer; how many are ready; "Nothing leaves this computer" or "Mail may leave: ‹account› → ‹runtime›".
- **Chain list:**
  - each row: order arrows (keyboard-reachable buttons, not drag-only), icon, name, one-line description or "Signed in as …", status tag, where tag, a primary action (Sign In / Add API Key / How to Install / Start), an on/off switch, and Details;
  - status vocabulary: Ready · Signed out · Needs an API key · Sign-in expired · Not installed · Not running · Looking… · Can't use this sign-in (subscription) · Broken (couldn't run `--version`).
- **Details panel:** Found (path, version, how it runs), Sign-in (owner-specific copy from §6.1), Providers (compat), Model, **What it may do** (locked text-only statement), runtime-specific notes (ChatGPT plan terms; Codex tool caveat), and Check ("Test with a short question" — one sentence, no mail).
- **When a runtime can't answer:** three radio options (§9).
- **Accounts matrix:** rows are accounts; columns are runtimes. "Stays here" for local; "Not allowed" / "Allowed · Stop" for `leaves`. The consent dialog states what is sent, to whom, what it can do, and how to stop. **Default focus is Don't Allow.**
- **Dialogs:**
  - Sign in with ChatGPT (browser, waiting, done);
  - Add API key (password field, keychain note, test);
  - CLI sign-in (waiting with progress, poll, cancel);
  - Add OpenAI-compatible provider (existing behaviour).
- All strings go through `tl!`. Icons are drawn in `icons.rs`: no vendor logos; vendor names appear only as text.

---

## 12. Privacy and documentation changes

- **PRIVACY.md:** a section per runtime type saying what is sent, to whom, under whose account, and where credentials live. State plainly that Claude subscriptions are not used, and why.
- **docs/threat-model.md:** "Local coding agents as text models": tool containment, kill-on-tool-event, environment allow-list, empty cwd, the Codex residual risk, and UAR external instances.
- **AGENTS.md:** no change needed. The existing rules cover subprocesses (Rust spawns them, no Node in the app) and foreign OAuth ids.
- **ATTRIBUTION.md:** only if `uar-sidecar` is ever bundled (G-RT7).

---

## 13. Scenarios (freeze ids in the phase contract)

| Id | Scenario | Kind |
|---|---|---|
| SCN-RT-DETECT-001 | Fake `claude`/`codex`/`opencode` scripts on a temp PATH: found, versions parsed; a group-writable one is refused | deterministic |
| SCN-RT-DETECT-002 | Hostile `--version` output (10 MB, control chars, no semver) → capped, `Broken`, no panic | deterministic |
| SCN-RT-AUTH-001 | `claude auth status` fixtures: each `authMethod`, plus OAuth logins with each billing value (Console/API, Pro, Max, Team, Enterprise, missing field) → usable / declined / needs-sign-in exactly per §6.3, including the fixture produced right after `claude auth login --console` | deterministic |
| SCN-RT-AUTH-002 | Codex isolation: with a fake owner `~/.codex/auth.json` present, a Codex run gets fresh per-run `CODEX_HOME` and `HOME` and only `CODEX_API_KEY` from MailCraft's store; the fake runtime records its environment and the paths it opened, and neither the owner's `~/.codex` nor `codex login` is ever touched | deterministic |
| SCN-RT-SIWC-001 | Fake OAuth server on loopback: wrong `state` rejected; correct flow stores the tokens in a fake credential store only; nothing in `ai.json` | deterministic |
| SCN-RT-SIWC-002 | Refresh at expiry−60 s; concurrent refreshes coalesce; `invalid_grant` / `invalid_client` / revocation → entry cleared and runtime off; **negative controls:** 429 with `Retry-After`, 408, a 400 without a grant error, and 503 → entry kept, retried with backoff | deterministic |
| SCN-RT-CONTAIN-001 | For **each stream format** (Claude stream-json, Codex JSONL, OpenCode, AG-UI), one fixture per tool class (tool use, command, file change, MCP call, web search) **plus one unknown non-text event kind**, emitted mid-stream. The fake runtime first spawns a grandchild that writes a heartbeat file every 50 ms. Assert: heartbeat stops within 1 s (whole group killed); no answer text shown; the answer pane shows the "tried to use a tool" notice **before** any fallback run starts. The scenario fails if any fixture class is missing. | deterministic |
| SCN-RT-WHERE-001 | `Where` derived for every runtime type and configuration in §2 (including a managed UAR with a verified loopback model → `local`, with any other model → `leaves`, external UAR → `leaves`); a runtime claiming "local" in its own output doesn't change it | deterministic |
| SCN-RT-ARGV-001 | For each spawned runtime, the fake records its argv: no prompt text, mail text, key or token appears in it | deterministic |
| SCN-RT-PATH-001 | Hostile `ai.json` path overrides: a relative path, a symlink to a world-writable file, a file in a world-writable directory, a non-regular file → `Broken`, nothing spawned | deterministic |
| SCN-RT-SANDBOX-001 | Under the OS sandbox, a fake runtime tries to read a sentinel file in the owner's home and to write outside the temp folder → both denied. Where no sandbox is available, Codex and OpenCode refuse to start | deterministic (per platform) |
| SCN-RT-BADGE-001 | Part A narrowing badge (A 5.7) names the runtime that would answer, for each runtime type | deterministic |
| SCN-RT-CONTAIN-002 | Environment of the spawned process (fake runtime dumps env) contains only the allow-list + its own credential var | deterministic |
| SCN-RT-CHAIN-001 | Chain with an account lacking consent for the first `leaves` runtime → it is skipped, the next eligible one answers, provenance lists both | deterministic |
| SCN-RT-CHAIN-002 | The same failing chain under `ask` (the question names the failed runtime and the next one; no second run starts until the owner answers) and under `stop` (the error is shown; no second run starts) | deterministic |
| SCN-RT-CHIP-001 | Status chip (Part A 5.4, B5.11) shows: "AI off" with no enabled runtime; "AI: on this computer" with only `local` runtimes enabled; "AI: may leave for ‹accounts›" when a `leaves` runtime has consent; "AI: on this computer" when a `leaves` runtime is enabled but no account consented | deterministic |
| SCN-RT-SETTINGS-001 | Hostile `ai.json` v3: a file over 256 KB; an unknown runtime kind in `chain`; `version` 99 or a string; junk in `consents` and `keys_kept_for`; a duplicate chain entry → the file loads safely, the unknown entry is dropped and never resolves to `local`, nothing panics | deterministic |
| SCN-RT-MIGRATE-001 | Every v2 → v3 migration row (all four primary × backup combinations, credential-entry renaming with read-back), plus malformed v2 | deterministic |
| SCN-RT-UAR-001 | Fake sidecar: wrong/absent `READY` → refused; correct handshake; requests carry the bearer; stdin close stops it | deterministic |
| SCN-RT-PRIV-001 | **Owner-run, owner-gated** (needs sudo once): capture on all interfaces except loopback (`tcpdump -i any not host 127.0.0.1 and not host ::1`, or `pf` logging on macOS) while MailCraft answers one question with only `on_device` + compat-local enabled. Assert: zero packets from MailCraft or its process group. If the owner declines, it is recorded as "not run", as for SCN-RT-LIVE-001 | live |
| SCN-RT-LIVE-001 | Owner-run, optional: one real question through each installed runtime with the owner's own credentials | live, owner-gated |

All fake runtimes are scripts written by the test into a temp dir. No real mailbox data is used (constraint `no-real-mailbox-in-tests`).
