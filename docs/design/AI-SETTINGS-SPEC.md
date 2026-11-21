# AI assistance settings: agent runtimes, configuration and sign-in

**Status:** design spec for the agent runtimes (see `PLAN.md` Part B), **amended by the owner's gate answers** recorded in the `mailcraft-ui-refresh-and-agent-runtimes` phase (G-RT2 b, G-RT3 a, G-RT5 b, G-RT6 b, G-RT12 as the owner specified it, and "no per-token billing, ever") and by that phase's entry checks on the installed CLIs (Claude Code 2.1.326, Codex 0.169.0, `universal-agent-runtime` 1.4.0-rc.5). Sections marked **Amended** say what was built; where they and older text disagree, the amendment wins. Folded in by change `ai-runtime-docs` (task 22.3).
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
  6. ~~OpenCode~~ (**cut**, G-RT5 b);
  7. Universal Agent Runtime (UAR).
- An **ordered chain** of runtimes, replacing primary + backup, with a stated fallback policy.
- **Detection** of installed runtimes, with a version and a plain status.
- **Sign-in automation:**
  - MailCraft runs the OAuth flow itself for ChatGPT plan.
  - **Amended:** for Claude Code, MailCraft opens the owner's terminal on the CLI's own `claude auth login` and watches its status; for Codex, the owner runs `codex login` (MailCraft never runs it).
  - API keys are stored once in the OS credential store, for `compat` providers only: no runtime needs one for Claude Code or Codex.
- **Token management:**
  - refresh, expiry and revocation for tokens MailCraft owns;
  - status-only for tokens a CLI owns, which MailCraft never reads.
- Every existing rule still holds: local-first, per-account consent for anything that leaves the computer, mail is data never instructions, no secrets in files or logs.

**Non-goals**
- Tool use by any runtime on mail. Every runtime is used as a **text model**.
- Installing or updating CLIs. MailCraft detects; the owner installs. This differs from The Boss, which downloads binaries.
- ~~Using a Claude Pro/Max subscription.~~ **Amended (G-RT2 b):** the owner signs in to the **unmodified** Claude Code CLI with their own login, a subscription included, as Anthropic's terms allow for an end user (read 2026-11-17). MailCraft never reads, stores, passes or intermediates that login and offers no claude.ai login of its own.
- Billing by the token for Claude Code or Codex: both run on the owner's own plan login and its quota.
- Reusing another program's OAuth client identity (constraint `no-foreign-oauth-client-ids`).
- Runtimes in the browser (wasm) build. It keeps OpenAI-compatible HTTP only.

---

## 2. Concepts

| Term | Meaning |
|---|---|
| **Runtime** | One way of getting an answer: `on_device`, `compat:<provider-id>`, `chatgpt`, `claude_code`, `codex`, `uar` (`opencode` is cut: an `ai.json` naming it has the entry dropped, never taken as local). |
| **Where** | `local` (text never leaves the computer) or `leaves` (it goes to a vendor or service). **Fixed per runtime type, never self-reported:** `on_device` = `local`; `compat` = from the provider kind (Local → `local`; Remote and Gateway → `leaves`); `chatgpt`, `claude_code`, `codex` = **always `leaves`** (each sends text to a vendor model under the owner's account); `uar` = `leaves`, except a **managed** instance whose only model endpoint MailCraft itself configured as a verified loopback `compat:Local` provider (§7.6). A runtime's own claim never makes it `local`. |
| **Chain** | The owner's ordered list of runtimes; each is on or off. |
| **Consent** | Per account × per runtime yes, required when *where* = `leaves`. This is the existing `set_consent`, generalised from provider ids to runtime ids. |
| **Credential owner** | `mailcraft`: MailCraft holds the secret (compat API keys and the SIWC entry in the OS credential store; the SIWC access token and the UAR launch token in memory). `runtime`: the CLI holds it (Claude Code, Codex) and MailCraft never reads it. |
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
      "path": null },                 // owner override of the binary path, or null = detected
    { "runtime": "codex",     "enabled": false, "path": null },
    { "runtime": "uar",       "enabled": false, "instance": "external" }  // "managed" | "external"
  ],
  "on_device": { "address": "http://127.0.0.1:11434", "identity": "…", "context_window": 4096 },
  "on_device_ports": [11434],
  "providers": [ /* unchanged ProviderConfig list for compat:* */ ],
  "consents": {                       // account address -> runtime ids allowed to receive its mail
    "avery@lumen.example": ["chatgpt"]
  },
  "chatgpt": { "ext_agent_host_id": "…43 base64url characters…" },  // not a secret
  "keys_kept_for": ["compat:openrouter", "chatgpt"],
  "detected": {                       // cache only; re-derived on rescan; never trusted for decisions
    "claude_code": { "path": "/Users/…/.local/bin/claude", "version": "2.1.326", "checked": "2026-11-17T10:00:00Z" }
  }
}
```

**Amended (as built, `apps/mailcraft/src/ai/settings_v3.rs`):**
- No chain entry has an `auth` field: Claude Code and Codex use the CLI's own login. An `auth` member in a file is ignored, never kept.
- There is no `opencode` runtime. An entry naming it (or any unknown kind) is dropped and never resolves to `local`.
- `chatgpt.ext_agent_host_id` is the installation's SIWC host id (32 random bytes, base64url, 43 characters), made once at the first sign-in; kept only when it is 43 base64url characters. Nothing else of `chatgpt` is read or written here.
- `keys_kept_for` keeps `compat:<id>`, `chatgpt` and a key's version 2 name `ai-key:<id>` until it is moved; anything else (`claude_code`, `codex`, junk) is dropped, so nothing ever asks the credential store for a Claude Code or Codex key.
- A file whose `version` this one doesn't know (99, a string) is read as no settings, with nothing enabled that leaves the computer, and is never overwritten.

### 3.1 Migration from v2

| v2 | v3 |
|---|---|
| `primary: "<id>"` | `chain[0] = compat:<id>`, enabled |
| `backup: "apple-on-device"` + confirmed `on_device` binding | `chain[1] = on_device`, enabled; **on macOS, if the binding is confirmed, `on_device` moves to `chain[0]`** only if the owner accepts the one-time question (G-RT1) |
| primary explicitly None, **no** compat backup | chain with every entry disabled; `never_configured: false` |
| primary explicitly None **and** a compat backup | the backup becomes an enabled `compat:<id>` entry, first; every other entry is disabled. **Precedence: this row wins over the row above** |
| primary set, `backup` absent / null / `""` | primary as `chain[0]`; no backup entry |
| primary set **and** a compat backup | primary as `chain[0]`, backup as `chain[1]`, both enabled |
| credential entries `ai-key:<provider-id>` (v2 key names; **amended**, the v2 name as built) | copied to `ai.compat:<provider-id>`, never over a newer key already there,; the old entry is deleted **only after** the new one is written and read back; `keys_kept_for` lists both names until the old one is gone |
| `never_configured: true` | v3 `never_configured: true`; first-run behaviour per §8 |
| provider consents | `consents[account] += "compat:<id>"` |

- The migrated file is written once, and the v2 file is kept as `ai.v2.json` for one release, so a rollback still works.
- Tests cover every row of the table plus a malformed v2.

### 3.2 Where each thing lives (**Amended**)

| Data | Where | Why |
|---|---|---|
| Chain, models, paths, fallback, consents | `ai.json` | Owner settings |
| API keys (compat remote and gateway only) | OS credential store via `Services::secrets`, service `mailcraft`, entry `ai.compat:<provider-id>` | Constraint `secrets-never-persisted-or-logged` |
| SIWC issued **client id**, **refresh token**, account id | One credential-store entry `ai.chatgpt`. The **access token lives in memory only** and is re-derived from the refresh token after a restart ("access tokens never stored") | Constraint `secrets-never-persisted-or-logged` |
| SIWC `ext_agent_host_id`, and that an entry was written | `ai.json` (`chatgpt`) | Not a secret; it names the installation to OpenAI's dynamic registration |
| UAR launch token | Memory only, new for each spawn of a managed instance | It's a session secret |
| CLI-owned credentials (Claude Code's `~/.claude`, `~/.claude.json` and Keychain item; Codex's `CODEX_HOME/auth.json`) | **Never read by MailCraft.** Their paths go only into the sandbox profile so the CLI, in its own process, can read them | Status comes from `claude auth status`, or (Codex) from how a run ends |
| Detection cache | `ai.json` `detected` | Speeds up the Options page; re-derived on Look Again; never used for a decision |

---

## 4. Layering (AGENTS.md architecture)

| Layer | Owns | New modules (proposed) |
|---|---|---|
| `crates/model` (pure) | runtime ids, `Where`, chain types, consent checks | `model::ai::runtime` |
| `crates/ai` (L5, transport, no egui) | parsing CLI status JSON and event streams; SIWC token types and refresh decisions; containment arg/env builders; UAR wire types | `ai::runtime::{claude_code, codex, uar, siwc, responses, contain, detect}` (no `opencode`: cut) — pure builders and parsers; **each adapter change (B5.5–B5.7, B5.9; B5.8 cut) adds the fuzz target for its own stream parser**; B5.4 provides the shared fuzz harness and the tool-event detector interface |
| `apps/mailcraft` (host) | spawning processes; login-shell PATH; OAuth loopback listener; credential store; timers | `apps/mailcraft/src/ai/runtime/{detect, spawn, siwc, uar_sidecar}.rs` |
| `crates/engine` | `ai.*` command specs and agent classes (**engine-engineer only**); no I/O | new owner-only commands (§10) |
| `crates/ui-egui` | settings page, sign-in dialogs, status | `ai/settings_runtimes.rs`, `ai/signin.rs` |

- The engine never spawns or connects. That's the existing request/apply pattern: the engine asks, the host runs `Host::run_job`.
- **wasm:** the runtime modules are `cfg(not(target_arch = "wasm32"))` in the host. The model and parsers stay portable, so wasm keeps building.

---

## 5. Detection

### 5.1 Algorithm (host, on Options open, Look Again, and lazily before first use) (**Amended**)
1. **Get the login-shell PATH once per session.**
   - **macOS / Linux:** spawn `$SHELL -lc 'printf %s "$PATH"'`.
     - `$SHELL` must be an absolute path to an existing file in `/etc/shells`; otherwise use `/bin/sh`.
     - 10 s timeout; 64 KB cap on the output.
   - **Windows:** the registry PATH.

   GUI apps on macOS don't inherit the shell PATH (pattern from `the-boss:shellEnv.ts`).
2. **For each runtime, check candidates in order**:
   1. the owner's path override;
   2. the login-shell PATH, resolved in Rust by walking PATH entries (no `sh -c`). **Amended:** only absolute entries are walked; an empty entry, `.`, or a literal `~/…` is skipped, since it would make a lookup depend on a folder nobody chose;
   3. well-known locations:

| Runtime | Executable | Well-known locations |
|---|---|---|
| Claude Code | `claude` | `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `%USERPROFILE%\.local\bin` |
| Codex | `codex` | `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, npm global bin |
| ~~OpenCode~~ | — | **cut** (G-RT5 b) |
| UAR | `universal-agent-runtime` (**amended**) | `~/.uar/bin` only (not looked for on the PATH), plus the probe `GET http://127.0.0.1:1906/.well-known/agent-card.json` (2 s, 1 MB cap, loopback checked on the address connected to, no redirect) |
| apfel | `apfel` + its server | existing on-device discovery (`device.rs`) |

3. **Version probe:** `<exe> --version` with no shell, an empty cwd, `stdin=null`, a 5 s timeout and a 4 KB output cap. Parse the first semver-looking token; keep the raw string ≤ 64 chars. Failure gives `Broken`, not `Missing`.
4. **Auth status probe** (§6.3), only for runtimes that are found, and (**amended**) only for Claude Code: Codex has no status probe.
5. **Results:** `Detected { path, version, source: override|path|wellknown, checked_at }`, written to the cache.

### 5.2 Rules
- Detection **never** runs a runtime with a prompt, opens the network (except UAR's loopback probe), or reads credential files.
- Paths are shown in full in the detail panel. The executable must be a regular file. On Unix, MailCraft warns (and doesn't run it) if it is group- or world-writable.
- Detection runs at most once a minute unless the owner chooses Look Again.
- **The same executable checks run again at every spawn**, for detected paths and for owner overrides (`ai.json` is hostile input): an absolute path; a regular file after resolving symlinks; owned by the owner or root; not group- or world-writable; the same for its parent directory. A failure gives `Broken` with the reason, and nothing runs. Covered by SCN-RT-PATH-001.

---

## 6. Sign-in and token management

### 6.1 Who owns which credential (**Amended**)

| Runtime | Credential | Owner | How MailCraft gets status | How it signs in |
|---|---|---|---|---|
| on_device | none | — | `/v1/models` + `/health` identity (existing) | One-time confirm (existing) |
| compat (remote / gateway) | API key | MailCraft | key present in credential store; Test Connection | Key dialog (existing) |
| chatgpt | SIWC: issued client id, refresh token (credential store); access token (memory) | MailCraft | entry present and not terminally refused | **MailCraft's own OAuth flow with OpenAI's dynamic registration** (§6.2) |
| claude_code | The CLI's own login, whatever it is: a claude.ai subscription, a Console account, a key the CLI holds | runtime | `claude auth status --json` (§6.3) | The owner runs `claude auth login` in a terminal MailCraft opens with that fixed argv, or copies the command (§6.4). MailCraft offers no claude.ai login of its own |
| codex | The owner's own ChatGPT login in the owner's `CODEX_HOME` (default `~/.codex`) | runtime | No probe: found and on is "not checked yet"; a run that fails for want of a sign-in is `SignedOut` | The owner runs `codex login` in a terminal; MailCraft never runs it, never reads `auth.json` and holds no Codex key (G-RT6 b, G-RT12 owner-specified) |
| ~~opencode~~ | — | — | — | **cut** (G-RT5 b) |
| uar | per-spawn launch token (managed only); UAR's own model credentials | MailCraft (launch token) / runtime (model keys) | managed: `READY:<port>` + capability check; external: the agent card probe | Managed: start the sidecar (§7.6), offered only when the installed program passes the entry check; external: the owner chooses it |

### 6.2 ChatGPT plan: Sign in with ChatGPT (MailCraft-owned OAuth) (**Amended**, G-RT3 a)

**No build-time client id and no owner registration.** MailCraft uses OpenAI's open-source dynamic per-account registration: the first authorize request names `client_id=dynamic_agent_client`, `agent_name_hint=MailCraft` and the installation's `ext_agent_host_id` (32 random bytes, base64url, made once and kept in `ai.json`; not a secret). OpenAI issues a client id for this owner and workspace in the callback. No client id is built in, read from settings or borrowed, which satisfies `no-foreign-oauth-client-ids`. Public client: PKCE, no client secret. Code: `crates/net/src/oauth/openai.rs` (on the same audited PKCE and loopback code as Microsoft's sign-in), `apps/mailcraft/src/ai/runtime/siwc.rs`, `crates/ai/src/runtime/siwc.rs`.

**Flow (host):**
1. A PKCE verifier and S256 challenge, a random `state`.
2. **Bind the loopback listener first:** `127.0.0.1`, a port the system chooses.
3. Open `https://auth.openai.com/api/accounts/authorize` in the default browser (through the window's opener, never a webview) with the parameters above, the scopes `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, and the redirect exactly `http://127.0.0.1:<port>/siwc/callback` (never `localhost`).
4. **Callback handling.** Exactly one request to `/siwc/callback` is acted on: the `state` is compared in constant time, and a wrong one ends the sign-in with nothing traded. Anything else that knocks is answered and the wait goes on. The reply is a static page with no external resources. At most 10 minutes; Cancel stops it; the port is closed however it ends. The callback's `code`, `state` and `client_id` are capped and held to a safe set of characters.
5. **Token exchange** at `https://auth.openai.com/oauth/token`, TLS only, with the issued client id, the code and the verifier; the answer capped. The issued client id, the refresh token and the account id go to the credential entry `ai.chatgpt` through `Services::secrets`, read back before the sign-in counts; the access token (about an hour) stays in memory. `ai.json` gets the host id and the flag that an entry was written, never the client id or a token. After a restart the first request refreshes first.
6. Show "Signed in as ‹email› (‹plan›)", from the ID token's claims, as untrusted text (capped at 128 characters, control characters removed; its signature isn't checked, it only fills in what is shown). The account id and plan are read from the claim `https://api.openai.com/auth` (`chatgpt_account_id`, `chatgpt_plan_type`), empty when absent; no `ChatGPT-Account-Id` header is sent. *Recorded risk: that claim's layout is assumed; verify at the live boundary.*

**Refresh:** when there is no access token in memory or it runs out within 60 s, or once after an HTTP 401. One refresh at a time per runtime; callers that find one under way wait for it. The rotated refresh token is written only if the entry still holds the one the refresh started with (compare-and-set). **Terminal:** `invalid_grant`, `invalid_client`, `unauthorized_client`: delete the entry, turn the runtime off, tell the owner "OpenAI ended this sign-in. Sign in with ChatGPT again to keep using your ChatGPT plan." **Transient** (entry kept, backoff): 408, 429 (at least its `Retry-After`, read as seconds, at most an hour), any other 4xx without a terminal grant error, 5xx and network errors. A damaged entry reads as not signed in and is not deleted.

**Sign out:** local deletion only. Delete the entry, drop the access token, turn the runtime off and take `chatgpt` out of every account's consents. Nothing is sent to OpenAI, which documents no revocation endpoint. The owner is told: "MailCraft has forgotten this ChatGPT sign-in on this computer. You can also remove MailCraft from the connected apps in your ChatGPT settings."

**Requests:** `POST https://api.openai.com/v1/responses` with `store: false`, `stream: true`, the owner's model, MailCraft's structured prompt in `instructions` and **no system or developer items**, and the question and the mail as one user `input` item inside a delimited data block. Never `tools`, `tool_choice`, `parallel_tool_calls`, `temperature`, `max_output_tokens`, `metadata` or `user`: the builder can't say them and a check refuses a body that does (`crates/ai/src/runtime/responses.rs`). The stream is read through `mailcraft_net::sse` with caps. Text deltas and the final text are the answer; **any non-text output item other than reasoning stops the run** (a reasoning item acts on nothing; reasoning models write one with every answer), as does any unknown event kind, a failed or cut-short response, or an `error` event. Refusal deltas are answer text.

### 6.3 Status probes for CLI-owned sign-ins (never read secrets) (**Amended**, G-RT2 b)

| Runtime | Command | Parse | States |
|---|---|---|---|
| Claude Code | `claude auth status --json` (pretty JSON on several lines; exit 1 when signed out) | Only `loggedIn`, `authMethod`, `subscriptionType` and `planTier` are read; the account's e-mail and organisation are skipped without being kept. Over 16 KB, not a JSON object, or without a boolean `loggedIn` is `Broken` | **Usable:** `loggedIn: true` with **any** `authMethod`, a claude.ai subscription included; shown as "uses your Claude plan" when `subscriptionType` or `planTier` names a plan. **NeedsSignIn:** `loggedIn: false`. There is no subscription-declined state |
| Codex | **none.** No status command is run (that would be `codex login status`) | — | Found and on: not checked yet. A run whose failed turn says it isn't signed in: `SignedOut`, whose words tell the owner to run `codex login`. A Codex that isn't signed in still contacts `api.openai.com` without a token before it fails |
| ~~OpenCode~~ | — | — | **cut** |

- The Claude probe runs through the same spawner, environment and sandbox as a run (§7.1), with no shell, a 5 s timeout and an empty cwd; its result is believed for 5 minutes, and asked again after a run that fails with a sign-in error.
- MailCraft never reads `~/.claude`, `~/.claude.json` or the Keychain item, and never any token.
- The output is hostile: parsed with caps; raw output is never shown.

### 6.4 Starting a CLI's own sign-in ("automate") (**Amended**)

- **Claude Code:** MailCraft opens the owner's terminal with a **fixed argv** that runs `claude auth login` (no string from settings, mail or a runtime reaches it; no shell starts it; the terminal gets MailCraft's environment without any variable that carries a key, a token or a vendor's settings). When no terminal can be opened, the command is shown to copy. Then `claude auth status` is asked every 2 s for at most 10 minutes, until it is usable or the owner cancels (`apps/mailcraft/src/ai/runtime/terminal.rs`). MailCraft never spawns `claude auth login` itself and offers no claude.ai login. Sign-out is the owner running `claude auth logout`; `ai.signOut` only says so.
- **Codex:** no sign-in is started. `ai.signIn` answers `{runYourself: "codex login"}` and spawns nothing; `ai.signOut` answers `{runYourself: "codex logout"}`. The argv of every Codex spawn is checked, and one holding `login` or `logout` is refused.
- **ChatGPT plan:** §6.2.
- ~~OpenCode~~: cut.
- **Never** pass a key on a command line, and never write anything into a CLI's config files.

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
  - **Amended HOME rule:** Claude Code gets the owner's **real `HOME`** (its login lookup depends on it: a per-run HOME with `CLAUDE_CONFIG_DIR` pointing at the real `~/.claude` reports "Not logged in"). Codex and a managed UAR get a **fresh per-run `HOME`** inside the run folder; Codex also gets `CODEX_HOME` set to the owner's Codex home (the absolute `CODEX_HOME` of MailCraft's own environment, else `~/.codex`), the one `CODEX_*` variable a recipe may name.
  - Allowed: `PATH` (login-shell PATH), `HOME`, `USER`, `LANG`, `TMPDIR`/`TEMP` (the run's own temporary folder), plus at most 16 variables the recipe names. **Amended:** no recipe names a credential variable.
  - **Never, whatever a recipe says:** `ANTHROPIC_*`, `OPENAI_*`, `CODEX_*` (but `CODEX_HOME`), `CLAUDE_*`, `UAR_*`, `LLM_*`, `NODE_OPTIONS`, `LD_PRELOAD` and the proxy variables (`crates/ai/src/runtime/contain.rs`, `is_forbidden_env_name`).
- **Output:** parsed as hostile JSON lines with per-line caps. Only text deltas and the final text are used. **Any tool-use, command, file or MCP event kills the run** and is reported as "The runtime tried to use a tool; MailCraft stopped it". The event *kind* is logged, never its content.
- **Prompt:** MailCraft's existing structured prompt (injection defences, mail as delimited data, citation rules), with the runtime's own system prompt replaced where possible.
- **Argv never carries prompt or mail text.** The prompt goes on stdin. Instructions go in a 0600 file inside the run's 0700 temp folder, passed by path (Claude Code: `--system-prompt-file`), or (Codex, whose `exec` has no flag that takes the file's path as an argument of its own) ahead of the prompt on stdin as one delimited block that can't close early. A spawn whose argv shares 32 bytes with the prompt, the instructions, a mail text or a secret is refused.
- **OS sandbox (preventive, not just detective).** Kill-on-tool-event happens *after* a tool ran, and a read file may already have gone to the vendor. Every spawned runtime therefore runs under an OS sandbox when one is available. It is denied reads outside its install directory, its own config/credential directory and the run's temp folder, and denied writes outside the temp folder. Network is allowed (the runtime must reach its vendor).
  - macOS: `sandbox-exec` with a generated Seatbelt profile (verify it is still usable for this at assess).
  - Linux: Landlock, or bubblewrap when present.
  - Windows: **none** (an AppContainer is deferred, SB-c).
  - **Amended:** `sandbox-exec` is deprecated by Apple (a recorded risk). The profile limits files only: network, Mach services (the keychain's among them) and processes are left as they are, and metadata stays readable. **Where no sandbox is available, Codex refuses to run** (`no_sandbox`), so it is not offered on Windows; Claude Code (`--tools ""` removes its tools before any run) and UAR (native tools off) may still run, with the residual risk named in the threat model.
- **Residual risk to state honestly (threat model, PRIVACY.md):** a runtime that is talked into reading a file it *can* read sends that text to its vendor before MailCraft sees any event. The sandbox limits what it can read. Kill-on-tool-event limits what happens next. Neither makes text exfiltration impossible.

### 7.2 Claude Code (**Amended**: containment option i, G-RT2 b)

`claude -p --tools "" --strict-mcp-config --setting-sources "" --no-session-persistence --output-format stream-json --verbose --safe-mode --system-prompt-file <run-dir>/instructions [--model <m>]`, the prompt on stdin (`crates/ai/src/runtime/claude_code.rs`, `RUN_FLAGS`).
- **The real `HOME`**, the env allow-list and nothing else (no `ANTHROPIC_*`, no `CLAUDE_*`, no `CLAUDE_CONFIG_DIR`); an empty 0700 cwd.
- `--safe-mode` (W7 entry check): with it, memory paths are empty and custom agents vanish; `--setting-sources ""` alone already kept the owner's skills and agents out, and the owner's `CLAUDE.md` didn't reach the prompt in either mode. The CLI's own login keeps working.
- **Sandbox:** reads of the CLI's install folder, `~/.claude`, `~/.claude.json` and, on macOS, `~/Library/Keychains` (without it the CLI reports "Not logged in"); writes to the run folder, `~/.claude` and the keychain folder (so a refreshed login is kept), and the CLI's per-user folders under `/tmp` (`claude-<uid>`, `cc-socks…`, `cc-daemon-<uid>`), shared with the owner's other Claude Code sessions (it ignores `TMPDIR`, and `CLAUDE_CODE_TMPDIR` may not reach a runtime). `~/.claude.json` writes aren't allowed (the CLI rewrites it through a temporary file beside it in HOME); runs work without them. Nothing else of HOME.
- **The init event.** One whose `tools` or `mcp_servers` list isn't empty kills the run (the skills, agents and slash-command name lists don't). *Deviation from "before the prompt is sent":* Claude Code 2.1.326 writes `init` only after reading its first input, so the prompt is written at once (`Gate::Now`, provisional pending the owner); an init listing a tool or MCP server still kills the run before any of its answer is kept. The `AfterInit` gate stays built and tested.
- Kill on any block that isn't text or thinking. A "not logged in" stream is `signed_out`; a vendor refusal (429, 402, the plan's quota) is `refused` with its status.

### 7.3 Codex (**Amended**: G-RT6 b, G-RT12 owner-specified)

`codex exec --json --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules --disable shell_tool -s read-only [-m <model>] -`, the instructions and then the prompt on stdin (`crates/ai/src/runtime/codex.rs`). **No `-a`** (not an `exec` option). Never `login` or `logout`.
- Environment: the allow-list, a **per-run `HOME`**, and **`CODEX_HOME` = the owner's Codex home**, where the owner's own ChatGPT login lives. No `CODEX_API_KEY`, no `OPENAI_*`. MailCraft reads nothing inside `CODEX_HOME`.
- `--ignore-user-config` keeps the owner's `config.toml`, and its MCP servers, out (checked against the real binary).
- **Contamination residual:** no flag of Codex 0.169.0 disables the skills and agent roles in `CODEX_HOME` (the features list, `multi_agent`, `skip_host_skill_discovery`, plugins, apps, hooks and `skill_search` were tried). Their text, and a `CODEX_HOME/AGENTS.md`, may reach the prompt sent to OpenAI; PRIVACY.md and the threat model say so. Codex writes an error item for every malformed agent role and dropped skill; those are noted by kind only.
- **Sandbox required:** reads of its install folder and `CODEX_HOME`; writes to the run folder and `CODEX_HOME` (its state databases, logs and caches, and a refreshed login). No sandbox: not run. Not offered on Windows.
- Kill on a command, a file change, an MCP call, a web search, a plan (`todo_list`) or any unknown item or event. A top-level `error` event (a transient reconnect) is noted as a failure, not a kill; an unclassified failure is `refused` with status 0.
- **Off by default.** Settings shows the tool caveat (research §5).

### 7.4 OpenCode (**Deferred**: cut by G-RT5 b)

OpenCode is not built in this phase (PLAN.md row B5.8 is cut; its fixtures are removed from SCN-RT-CONTAIN-001). An `ai.json` entry for `opencode` is dropped when read and never taken as local. The earlier recipe (an `opencode serve` on loopback with a deny-all permissions config) is kept in the history of this file for a later phase that can prove the deny config.

### 7.5 ChatGPT plan, compat, on_device
In-process HTTP through `crates/ai`'s client (existing), never with `tools`; the ChatGPT plan through `crates/ai/src/runtime/responses.rs` and `mailcraft_net` (§6.2).

### 7.6 UAR
**Managed instance:**
0. **Amended (entry check):** offered only when the installed program's `--help` shows all four of a config path, a port, a launch token read on stdin and a readiness line. `universal-agent-runtime` 1.4.0-rc.5 at `~/.uar/bin` shows the first two only, so for it UAR is **external-only**; the managed path is built and tested against a fake. No argv token, fixed shared port or polled readiness is taken in their place.
1. Spawn `universal-agent-runtime --config <MailCraft-owned config> --port 0` with the sanitized environment and a config that disables native file, web and terminal tools.
2. Write a 32-byte launch token on stdin.
3. Await `READY:<port>` within 20 s.
4. Check the required capabilities. **`Where` for a managed instance:** MailCraft writes UAR's model configuration itself. It is `local` only if that configuration names exactly one model endpoint: a MailCraft `compat` provider of kind Local, whose resolved peer MailCraft verified as loopback (operator decision D7). Any other configuration, or any model the instance reports beyond it, makes it `leaves`. UAR's own reports are never trusted for this.
5. Requests go to `127.0.0.1:<port>` with `Authorization: Bearer <token>`; runs via `POST /api/uar/runs`, events from the AG-UI stream. Only text-message events are used; tool-call events cancel the run.
6. Shutdown by closing stdin; a generation counter rejects stale requests.

**External instance (already running on 1906):** used only if the owner chooses it. MailCraft can't contain an external instance's tools, so Settings says so and it counts as `leaves`. **Amended:** MailCraft never reads UAR's credential or config files, so it sends no bearer to an external instance; a 401 or 403 is `uar_refused`. A tool-call AG-UI event still kills the run.

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
| `ai.setRuntimeOption` | `{runtime, model?, path?}` (**amended**: no `auth`) | validated per runtime |
| `ai.rescan` | — | detection now |
| `ai.signIn` | `{runtime}` | starts §6.2 / §6.4; answers `{pending: "signIn"}`; for Codex answers `{runYourself: "codex login"}` and starts nothing |
| `ai.signOut` | `{runtime}` | §6.2 sign-out, or forget the MailCraft-held key; never touches CLI stores (Claude Code and Codex answer `{runYourself: "claude auth logout" / "codex logout"}`) |
| `ai.setRuntimeKey` | `{runtime}` | opens the key dialog for a `compat` provider in the chain; refused for every other runtime; keys are never accepted as a parameter (control channel or MCP) |
| `ai.setConsent` | `{runtime, account, allowed}` | existing, widened from provider id to runtime id |
| `ai.setFallback` | `{value: next|ask|stop}` | |

- All are added to `hostile_params_never_panic`.
- Control-channel exposure for owner scripts follows the existing `control/ai.rs` pattern (refused to agents at `agent_gate.rs:99`).

---

## 11. UI (maps to `settings.html`)

- **Summary chips:** first runtime that can answer; how many are ready; "Nothing leaves this computer" or "Mail may leave: ‹account› → ‹runtime›". **Amended (owner, W3):** with a `leaves` runtime or provider active and no account consented, the status chip says "AI: your typed searches go to ‹host›" (host name only), never "AI: on this computer".
- **Chain list:**
  - each row: order arrows (keyboard-reachable buttons, not drag-only), icon, name, one-line description or "Signed in as …", status tag, where tag, a primary action (Sign In / Add API Key / How to Install / Start / Copy the sign-in command), an on/off switch, and Details;
  - status vocabulary (**amended**): Ready · Signed out · Needs an API key (compat only) · Sign-in expired · Not installed · Not running · Not checked yet (Codex) · Looking… · Not offered on this computer (Codex on Windows) · Broken (couldn't run `--version`). There is **no** "Can't use this sign-in (subscription)" state; Claude Code and Codex rows say "uses your Claude plan" / "uses your ChatGPT plan" and that MailCraft never reads the login.
- **Details panel:** Found (path, version, how it runs), Sign-in (owner-specific copy from §6.1), Providers (compat), Model, **What it may do** (locked text-only statement), runtime-specific notes (ChatGPT plan terms and the local-only sign-out with the pointer to ChatGPT settings; the plan-quota statement for Claude Code and Codex; Codex's tool caveat and its skills and agent-roles residual; the external UAR caveat), and Check ("Test with a short question" — one sentence, no mail).
- **When a runtime can't answer:** three radio options (§9).
- **Accounts matrix:** rows are accounts; columns are runtimes. "Stays here" for local; "Not allowed" / "Allowed · Stop" for `leaves`. The consent dialog states what is sent, to whom, what it can do, and how to stop. **Default focus is Don't Allow.**
- **Dialogs:**
  - Sign in with ChatGPT (browser, waiting, done);
  - Add API key (password field, keychain note, test), compat only;
  - CLI sign-in: Claude Code (terminal opened, waiting with progress, poll, cancel; or the command to copy); Codex (the command shown, never run);
  - Add OpenAI-compatible provider (existing behaviour).
- All strings go through `tl!`. Icons are drawn in `icons.rs`: no vendor logos; vendor names appear only as text.

---

## 12. Privacy and documentation changes

- **PRIVACY.md:** a section per runtime type saying what is sent, to whom, under whose account, and where credentials live. **Amended:** state plainly that Claude Code and Codex run on the owner's own plan login and its quota, that MailCraft never reads those logins and bills nothing by the token, and the contamination residuals the entry checks found (done in `ai-runtime-docs`).
- **docs/threat-model.md:** "Local coding agents as text models": tool containment, kill-on-tool-event, environment allow-list, empty cwd, the sandbox and its gaps, the Codex residual risk, and UAR external instances (done in `ai-runtime-docs`).
- **AGENTS.md:** no change needed. The existing rules cover subprocesses (Rust spawns them, no Node in the app) and foreign OAuth ids.
- **ATTRIBUTION.md:** only if UAR is ever bundled (G-RT7 a: it isn't; MailCraft detects the installed one).

---

## 13. Scenarios (frozen in the phase contract) (**Amended**)

| Id | Scenario | Kind |
|---|---|---|
| SCN-RT-DETECT-001 | Fake `claude`/`codex` scripts on a temp PATH and a fake `~/.uar/bin/universal-agent-runtime`: found, versions parsed; a group-writable one is refused; non-absolute PATH entries are skipped; only `--version` is ever run. **Amended:** no OpenCode | deterministic |
| SCN-RT-DETECT-002 | Hostile `--version` output (10 MB, control chars, no semver) → capped, `Broken`, no panic | deterministic |
| SCN-RT-AUTH-001 | **Amended:** `claude auth status` fixtures (every `authMethod`, a claude.ai subscription included, `api_key`, Console, a missing field): `loggedIn: true` with any method → usable, shown as "uses your Claude plan" when `subscriptionType`/`planTier` names a plan; `loggedIn: false` → needs sign-in. MailCraft makes zero reads of `~/.claude/**`, `.claude.json` or the Keychain item (audited under a sandbox profile that denies MailCraft's own process those paths), never spawns `claude auth login` itself and offers no claude.ai login | deterministic |
| SCN-RT-AUTH-002 | **Amended:** Codex on the owner's `CODEX_HOME`: the fake records argv, env and opened paths. Argv is exactly `exec --json --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules --disable shell_tool -s read-only -` (no `-a`); env has `CODEX_HOME` = the fixture owner home and no `CODEX_API_KEY`; MailCraft never opens `CODEX_HOME/auth.json` and never spawns `codex login`; with a fixture `config.toml` declaring an MCP server, none is loaded | deterministic |
| SCN-RT-SIWC-001 | **Amended (dynamic registration):** fake OAuth server on loopback. The authorize URL carries `client_id=dynamic_agent_client`, `agent_name_hint=MailCraft`, the persisted `ext_agent_host_id`, PKCE S256, `state`, the six scopes and the redirect exactly `http://127.0.0.1:<port>/siwc/callback`; a wrong `state` is rejected and nothing is stored; the correct flow stores the issued client id and the refresh token in a fake credential store only, the access token in memory only; `ai.json` holds the host id and no token; the listener binds 127.0.0.1 and closes after one request | deterministic |
| SCN-RT-SIWC-002 | Refresh at expiry−60 s; concurrent refreshes coalesce; `invalid_grant` / `invalid_client` / `unauthorized_client` → entry cleared and runtime off; **negative controls:** 429 with `Retry-After`, 408, a 400 without a grant error, and 503 → entry kept, retried with backoff. Sign-out is local deletion only | deterministic |
| SCN-RT-CONTAIN-001 | **Amended (OpenCode removed):** for **each stream format** (Claude stream-json, Codex JSONL, AG-UI), one fixture per tool class (tool use, command, file change, MCP call, web search) **plus one unknown non-text event kind**, emitted mid-stream. The fake runtime first spawns a grandchild that writes a heartbeat file every 50 ms. Assert: heartbeat stops within 1 s (whole group killed); no answer text shown; the answer pane shows the "tried to use a tool" notice **before** any fallback run starts; a Claude `init` event listing any tool or MCP server kills the run (before the prompt is written under `Gate::AfterInit`; with the provisional `Gate::Now` of §7.2, before any of its answer is kept). The scenario fails if any fixture class is missing. | deterministic |
| SCN-RT-WHERE-001 | `Where` derived for every runtime type and configuration in §2 (including a managed UAR with a verified loopback model → `local`, with any other model → `leaves`, external UAR → `leaves`); a runtime claiming "local" in its own output doesn't change it | deterministic |
| SCN-RT-ARGV-001 | For each spawned runtime, the fake records its argv: no prompt text, mail text, key or token appears in it | deterministic |
| SCN-RT-PATH-001 | Hostile `ai.json` path overrides: a relative path, a symlink to a world-writable file, a file in a world-writable directory, a non-regular file → `Broken`, nothing spawned | deterministic |
| SCN-RT-SANDBOX-001 | Under the OS sandbox, a fake runtime tries to read a sentinel file in the owner's home and to write outside the temp folder → both denied; reading the CLI's own allowed config path succeeds. Where no sandbox is available, Codex refuses to start; Claude Code may run with the residual risk recorded | deterministic (per platform) |
| SCN-RT-BADGE-001 | Part A narrowing badge (A 5.7) names the runtime that would answer, for each runtime type | deterministic |
| SCN-RT-CONTAIN-002 | Environment of the spawned process (fake runtime dumps env) contains only the allow-list + its own credential var | deterministic |
| SCN-RT-CHAIN-001 | Chain with an account lacking consent for the first `leaves` runtime → it is skipped, the next eligible one answers, provenance lists both | deterministic |
| SCN-RT-CHAIN-002 | The same failing chain under `ask` (the question names the failed runtime and the next one; no second run starts until the owner answers) and under `stop` (the error is shown; no second run starts) | deterministic |
| SCN-RT-CHIP-001 | Status chip (Part A 5.4, B5.11) shows: "AI off" with no enabled runtime; "AI: on this computer" with only `local` runtimes enabled; "AI: may leave for ‹accounts›" when a `leaves` runtime has consent. **Amended (owner, W3; contract amendment for SCN-CHIP-001):** with a `leaves` runtime or provider active and no account consented, "AI: your typed searches go to ‹host›" (host name only) | deterministic |
| SCN-RT-SETTINGS-001 | Hostile `ai.json` v3: a file over 256 KB; an unknown runtime kind in `chain`; `version` 99 or a string; junk in `consents` and `keys_kept_for`; a duplicate chain entry → the file loads safely, the unknown entry is dropped and never resolves to `local`, nothing panics | deterministic |
| SCN-RT-MIGRATE-001 | Every v2 → v3 migration row (all four primary × backup combinations, credential-entry renaming with read-back), plus malformed v2 | deterministic |
| SCN-RT-UAR-001 | Fake sidecar (**amended**: `universal-agent-runtime`, probed with `/.well-known/agent-card.json`): wrong/absent `READY` within 20 s → refused and killed; correct handshake; requests carry the bearer; stdin close stops it; a stale generation is rejected; tool-call AG-UI events cancel the run. The installed 1.4.0-rc.5 fails the managed entry check, so it is offered external-only | deterministic |
| SCN-RT-PRIV-001 | **Owner-run, owner-gated** (needs sudo once): capture on all interfaces except loopback (`tcpdump -i any not host 127.0.0.1 and not host ::1`, or `pf` logging on macOS) while MailCraft answers one question with only `on_device` + compat-local enabled. Assert: zero packets from MailCraft or its process group. If the owner declines, it is recorded as "not run", as for SCN-RT-LIVE-001 | live |
| SCN-RT-LIVE-001 | Owner-run: one synthetic question through each installed runtime on the **owner's own logins** (Claude Max, the ChatGPT plan), no per-token cost. Pass needs non-empty text, zero tool events, a Claude init with no tools or MCP, and no MailCraft read of CLI credentials; a 429, 5xx, network error or exhausted quota is retried once after 60 s, then BLOCKED, never FAIL | live, owner-gated |

**Frozen in the phase contract** with these: SCN-BASE-001…003, SCN-MEASURE-001, SCN-CHIP-001, SCN-AGENT-001…003, SCN-ENTER-001, SCN-A11Y-001, SCN-CONTRAST-001 (Part A and the baseline), and the earlier AI phase's live scenarios carried as regression scenarios: SCN-PRIOR-RETRIEVE-001, SEARCH-001, ANSWER-001, INJECT-001, PRIVACY-001, LOOPBACK-001, OPTIN-001, STREAM-001, FAIL-001, SAFETY-001 and MCP-001.

All fake runtimes are scripts written by the test into a temp dir. No real mailbox data is used (constraint `no-real-mailbox-in-tests`).
