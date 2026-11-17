# 6 — Execute one wave

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
   - **Part B OAuth:**
     - PKCE.
     - Bind the loopback listener before opening the browser.
     - Constant-time check of `state`.
     - Keep tokens only in the credential store.
     - Refresh with compare-and-set; a 4xx from the token endpoint is terminal.
3. **After the agents finish,** run the wave's **integration task** yourself, or with the named integrator:
   - `UI_COMMANDS`, dispatch, `enabled` rules and panel placement in `lib.rs`;
   - adapter registration;
   - `cargo xtask i18n`.
4. **Build and look** (if the owner chose build-per-wave at assess):
   - Run `cargo xtask ci`.
   - Capture `examples/ui_shot.rs` screenshots of the touched surfaces in light, dark, high contrast and 200 %, and look at them.
5. Update `tasks.md` and `progress.json`. Don't commit until the boundary unless the owner asked for per-wave commits. Stop after the wave.
