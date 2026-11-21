# 6 — Execute one wave

> **Run once already** for phase `mailcraft-ui-refresh-and-agent-runtimes`: its gate answers, final change ids and frozen scenario ids are in [README.md](README.md#the-record-of-the-phase-these-prompts-ran). OpenCode is cut (G-RT5 b).

Run `/kbd-execute` for `<PHASE>`, **wave `<N>` only**, as listed in `execution.md`.

1. Dispatch each change to its owning role, as routed in `plan.md`. Give every agent:
   - the change folder;
   - the relevant section of `docs/design/PLAN.md`, plus `AI-SETTINGS-SPEC.md` for Part B;
   - the file-ownership grant;
   - the containment and privacy rules that apply.
2. **Rules for every agent:**
   - **No panics:** no `unwrap`, `expect`, `panic!` or `unsafe` outside tests.
   - **Tests in their own files:** use `*_tests.rs`.
   - **Colours and text:** colours only from `Tokens`, and text through `tl!`.
   - **No secrets** in files, logs or argv.
   - **Owner-only commands:** every new command is `Never` for agents (engine-engineer decides), and is added to `hostile_params_never_panic`.
   - **Part B spawning:**
     - Spawn with argv only: no shell and no inherited environment.
     - Use an empty cwd.
     - Kill the whole process group on any tool event.
     - Fakes only in tests.
   - **Part B OAuth** (Sign in with ChatGPT, as amended):
     - PKCE; OpenAI's dynamic registration (`dynamic_agent_client`), no built-in client id.
     - Bind the loopback listener on 127.0.0.1 before opening the browser; the redirect is exactly `http://127.0.0.1:<port>/siwc/callback`.
     - Constant-time check of `state`.
     - The issued client id and the refresh token only in the credential store; the access token in memory only.
     - Refresh with compare-and-set; only `invalid_grant`, `invalid_client` and `unauthorized_client` are terminal. 408, 429, other 4xx, 5xx and network errors keep the entry.
   - **Part B CLIs:** never read a CLI's login, never run `claude auth login`, `codex login` or a logout, and never hand a runtime an API key.
3. **After the agents finish,** run the wave's **integration task** yourself, or with the named integrator:
   - `UI_COMMANDS`, dispatch, `enabled` rules and panel placement in `lib.rs`;
   - adapter registration;
   - `cargo xtask i18n`.
4. **Build and look** (if the owner chose build-per-wave at assess):
   - Run `cargo xtask ci`.
   - Capture `examples/ui_shot.rs` screenshots of the touched surfaces in light, dark, high contrast and 200 %, and look at them.
5. Update `tasks.md` and `progress.json`. Don't commit until the boundary unless the owner asked for per-wave commits. Stop after the wave.
