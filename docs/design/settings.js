/* AI assistance settings prototype. Scripted: nothing is detected, signed in or sent for real.
   All names, paths, versions and accounts are example data. */
(function () {
  "use strict";
  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const icon = (id, extra = "") => `<svg class="od-icon" ${extra} aria-hidden="true"><use href="#${id}"/></svg>`;

  const ACCOUNTS = [
    { id: "work", addr: "avery@lumen.example", label: "Work" },
    { id: "home", addr: "avery@quinnfamily.example", label: "Personal" }
  ];

  /* where: local = stays on this computer; sub = goes to the vendor under the owner's subscription; net = internet service */
  const state = {
    selected: null,
    fallback: "next",
    runtimes: [
      { id: "apple", name: "Apple's on-device model", icon: "i-chip", where: "local", on: true, status: "ready",
        sub: "Apple Intelligence on this Mac, through apfel",
        facts: { "Found": "apfel at /opt/homebrew/bin/apfel", "Address": "http://127.0.0.1:11434", "Model": "apple-foundationmodel", "Context": "4,096 tokens", "Confirmed": "Today, by you" },
        auth: "none", models: ["apple-foundationmodel"], model: "apple-foundationmodel" },
      { id: "chatgpt", name: "ChatGPT plan", icon: "i-cloud", where: "sub", on: false, status: "signedOut",
        sub: "Sign in with ChatGPT: your Go, Plus, Pro or Business plan pays for answers",
        facts: { "Talks to": "OpenAI's Responses API, directly", "Runs as": "MailCraft itself, text only, no tools" },
        auth: "siwc", vendor: "OpenAI", signedInAs: "", models: ["Default for your plan"], model: "Default for your plan" },
      { id: "claude", name: "Claude Code", icon: "i-term", where: "sub", on: false, status: "needsKey",
        sub: "Runs Claude Code with an Anthropic API key",
        facts: { "Found": "~/.local/bin/claude", "Version": "2.1.326", "Runs as": "claude -p, text only, no tools, no MCP, nothing saved" },
        auth: "apikey", vendor: "Anthropic", signedInAs: "", models: ["Claude Sonnet", "Claude Opus", "Claude Haiku"], model: "Claude Sonnet" },
      { id: "codex", name: "Codex CLI", icon: "i-term", where: "sub", on: false, status: "needsKey",
        sub: "Runs Codex with an OpenAI API key, sandboxed",
        facts: { "Found": "/opt/homebrew/bin/codex", "Version": "0.169.0", "Runs as": "codex exec --json, OS sandbox, empty folder and home, nothing saved" },
        auth: "apikey", vendor: "OpenAI", signedInAs: "", models: ["Codex default"], model: "Codex default" },
      { id: "compat", name: "OpenAI-compatible providers", icon: "i-cloud", where: "mixed", on: true, status: "ready",
        sub: "Ollama, LM Studio, llama.cpp, OpenRouter, a company gateway…",
        providers: [{ name: "Ollama", url: "http://localhost:11434/v1", where: "local", model: "llama3.2" }],
        auth: "key", models: ["llama3.2"], model: "llama3.2" },
      { id: "opencode", name: "OpenCode", icon: "i-term", where: "sub", on: false, status: "missing",
        sub: "Uses OpenCode's own provider sign-ins",
        facts: { "Looked in": "PATH, ~/.opencode/bin, /opt/homebrew/bin" }, auth: "own", vendor: "the provider OpenCode is set to", models: [], model: "" },
      { id: "uar", name: "Universal Agent Runtime", icon: "i-hub", where: "sub", on: false, status: "stopped",
        sub: "A runtime on this computer that hosts agents and routes them to models",
        facts: { "Found": "uar-sidecar (Prometheus Universal Agent Runtime)", "Listens on": "127.0.0.1:1906, started by MailCraft", "Runs as": "AG-UI runs, native file, web and terminal tools off" },
        auth: "own", vendor: "whichever model it routes to", models: ["Runtime default"], model: "Runtime default" }
    ],
    consent: { work: {}, home: {} }
  };

  const byId = (id) => state.runtimes.find((r) => r.id === id);
  const leaves = (r) => r.where === "sub" || r.where === "net" || (r.id === "compat" && r.providers.some((p) => p.where === "net"));

  /* ---------- toast ---------- */
  let tt = null;
  function toast(t) { const el = $("#toast"); el.textContent = t; el.hidden = false; clearTimeout(tt); tt = setTimeout(() => { el.hidden = true; }, 3200); }

  /* ---------- theme ---------- */
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  function setTheme(v) {
    document.documentElement.dataset.theme = v;
    document.documentElement.classList.toggle("sys-dark", v === "system" && mq.matches);
    document.querySelectorAll("[data-theme-set]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.themeSet === v)));
  }
  mq.addEventListener("change", () => setTheme(document.documentElement.dataset.theme));

  /* ---------- status vocabulary ---------- */
  function statusTag(r) {
    switch (r.status) {
      case "ready": return `<span class="tag ok">${icon("i-check")}Ready</span>`;
      case "signedOut": return `<span class="tag attn">${icon("i-warn")}Signed out</span>`;
      case "needsKey": return `<span class="tag attn">${icon("i-warn")}Needs an API key</span>`;
      case "expired": return `<span class="tag bad">${icon("i-warn")}Sign-in expired</span>`;
      case "missing": return `<span class="tag mute">Not installed</span>`;
      case "stopped": return `<span class="tag mute">Not running</span>`;
      case "checking": return `<span class="tag mute">Looking…</span>`;
      default: return "";
    }
  }
  function whereTag(r) {
    if (r.where === "local") return `<span class="tag where-local">${icon("i-chip")}On this computer</span>`;
    if (r.id === "compat") {
      const net = r.providers.filter((p) => p.where === "net").length;
      return net ? `<span class="tag where-net">${icon("i-cloud")}${net} on the internet</span><span class="tag where-local">${icon("i-chip")}${r.providers.length - net} on this computer</span>` : `<span class="tag where-local">${icon("i-chip")}On this computer</span>`;
    }
    return `<span class="tag where-sub">${icon("i-cloud")}Sends to ${esc(r.vendor)}</span>`;
  }

  /* ---------- summary ---------- */
  function renderSummary() {
    const first = state.runtimes.find((r) => r.on && r.status === "ready");
    const leaving = [];
    ACCOUNTS.forEach((a) => Object.entries(state.consent[a.id]).forEach(([rid, yes]) => { if (yes && byId(rid) && byId(rid).on) leaving.push(`${a.label} → ${byId(rid).name}`); }));
    const ready = state.runtimes.filter((r) => r.on && r.status === "ready").length;
    $("#summary").innerHTML =
      `<span class="tag ${first ? "ok" : "bad"}">${first ? `First answer from: ${esc(first.name)}` : "No runtime can answer"}</span>` +
      `<span class="tag mute">${ready} ready and on</span>` +
      (leaving.length ? `<span class="tag where-sub">${icon("i-cloud")}Mail may leave: ${esc(leaving.join(", "))}</span>` : `<span class="tag where-local">${icon("i-shield")}Nothing leaves this computer</span>`);
  }

  /* ---------- chain ---------- */
  function rowHtml(r, i, n) {
    const canToggle = r.status !== "missing";
    const action = r.status === "signedOut" || r.status === "expired" ? `<button class="btn quiet" type="button" data-signin="${r.id}">Sign In…</button>`
      : r.status === "missing" ? `<button class="btn quiet" type="button" data-install="${r.id}">How to Install</button>`
      : r.status === "stopped" ? `<button class="btn quiet" type="button" data-start="${r.id}">Start</button>` : "";
    return `<li class="rt${r.on ? "" : " is-off"}" aria-current="${state.selected === r.id}" data-id="${r.id}">
      <div class="rt-order">
        <button class="arrow" type="button" data-move="${r.id}" data-dir="-1" aria-label="Move ${esc(r.name)} up" ${i === 0 ? "disabled" : ""}>${icon("i-up")}</button>
        <span class="rt-num" aria-hidden="true">${i + 1}</span>
        <button class="arrow" type="button" data-move="${r.id}" data-dir="1" aria-label="Move ${esc(r.name)} down" ${i === n - 1 ? "disabled" : ""}>${icon("i-down")}</button>
      </div>
      <span class="rt-icon">${icon(r.icon)}</span>
      <button class="rt-main" type="button" data-open="${r.id}" style="text-align:left" aria-label="${esc(r.name)} details">
        <span class="rt-name od-truncate">${esc(r.name)}</span>
        <span class="rt-sub od-truncate">${esc(r.signedInAs ? `Signed in as ${r.signedInAs}` : r.sub)}</span>
        <span class="rt-tags">${statusTag(r)}${whereTag(r)}</span>
      </button>
      <div class="rt-side">
        ${action}
        <button class="switch" type="button" role="switch" aria-checked="${r.on}" data-toggle="${r.id}" aria-label="Use ${esc(r.name)}" ${canToggle ? "" : "disabled"}></button>
        <button class="arrow" type="button" data-open="${r.id}" aria-label="Open ${esc(r.name)} details">${icon("i-chev")}</button>
      </div>
    </li>`;
  }
  function renderChain() {
    const n = state.runtimes.length;
    $("#chain").innerHTML = state.runtimes.map((r, i) => rowHtml(r, i, n)).join("");
  }

  /* ---------- accounts matrix ---------- */
  function renderAccounts() {
    const cols = state.runtimes.filter((r) => r.status !== "missing");
    const head = `<div class="acct-row2" style="--cols:${cols.length}" role="row"><span>Account</span>${cols.map((r) => `<span class="od-truncate">${esc(r.name)}</span>`).join("")}</div>`;
    const rows = ACCOUNTS.map((a) => `<div class="acct-row2" style="--cols:${cols.length}" role="row"><span class="od-field"><strong>${esc(a.label)}</strong><span class="help od-truncate">${esc(a.addr)}</span></span>${cols.map((r) => {
      if (!leaves(r)) return `<span><span class="cell-btn local">${icon("i-chip")}Stays here</span></span>`;
      const yes = !!state.consent[a.id][r.id];
      return `<span><button type="button" class="cell-btn ${yes ? "yes" : "no"}" data-consent="${a.id}:${r.id}" aria-pressed="${yes}" aria-label="${esc(a.label)} mail to ${esc(r.name)}: ${yes ? "allowed, select to stop" : "not allowed, select to allow"}">${yes ? `${icon("i-cloud")}Allowed · Stop` : "Not allowed"}</button></span>`;
    }).join("")}</div>`).join("");
    $("#acctTable").innerHTML = head + rows;
  }

  /* ---------- detail ---------- */
  function renderDetail() {
    const panel = $("#detail");
    const r = byId(state.selected);
    if (!r) { panel.hidden = true; return; }
    panel.hidden = false;
    const facts = r.facts ? `<dl class="kv">${Object.entries(r.facts).map(([k, v]) => `<dt>${esc(k)}</dt><dd class="${/\/|~|http/.test(v) ? "mono" : ""}">${esc(v)}</dd>`).join("")}</dl>` : "";
    const signIn = (() => {
      if (r.auth === "none") return `<p class="help">No sign-in. It runs on this Mac and never sends text anywhere.</p>`;
      if (r.auth === "key") return "";
      const how = {
        chatgpt: "MailCraft signs in with OpenAI's Sign in with ChatGPT. It keeps the long-lived part of the sign-in in the system keychain and the short-lived part only in memory. They refresh on their own; if OpenAI ends the sign-in, this turns off and says so.",
        claude: "Claude Code is used only with API billing: an Anthropic API key you add here (kept in the keychain and handed to Claude Code for each request), or Claude Code signed in with a Console account. Anthropic's terms don't let other apps use a Claude Pro or Max subscription, so MailCraft won't use one.",
        codex: "Codex runs with an OpenAI API key that MailCraft keeps in the keychain and hands to Codex for each request, in a fresh, empty home folder. Codex's own sign-in isn't used. For your ChatGPT plan, use ChatGPT plan above, which is the sign-in OpenAI provides for other apps.",
        opencode: "OpenCode keeps its own provider sign-ins. MailCraft runs it with its own settings that deny every action.",
        uar: "The runtime is started by MailCraft with a one-time launch key, on this computer only. It uses the model credentials you gave it; MailCraft never sees them."
      }[r.id] || "";
      const state_ = r.signedInAs ? `<div class="success">${icon("i-check")}<div class="od-field"><strong>Signed in as ${esc(r.signedInAs)}</strong><span class="help">Checked just now. MailCraft checks again before each request.</span></div></div>` : "";
      const btns = r.signedInAs
        ? `<button class="btn quiet" type="button" data-signout="${r.id}">Sign Out of ${esc(r.name)}…</button>`
        : r.status === "missing" ? "" : `<button class="btn primary" type="button" data-signin="${r.id}">${r.auth === "apikey" ? "Add API Key…" : r.id === "chatgpt" ? "Sign in with ChatGPT…" : "Sign In…"}</button>`;
      const blocked = r.id === "claude" && !r.signedInAs ? `<div class="note warn">Claude Code on this Mac is signed in with a Claude subscription. MailCraft can't use that sign-in. Add an API key, or sign Claude Code in with a Console account.</div>` : "";
      return `${state_}${blocked}<p class="help">${how}</p><div class="btn-row">${btns}</div>`;
    })();
    const providers = r.id === "compat" ? `<section class="d-sec"><h4>Providers</h4><div class="prov-list">${r.providers.map((p, i) => `<div class="prov"><span class="od-field"><strong class="od-truncate">${esc(p.name)}</strong><span class="help mono od-truncate">${esc(p.url)}</span></span><span class="tag ${p.where === "local" ? "where-local" : "where-net"}">${p.where === "local" ? "This computer" : "Internet"}</span></div>`).join("")}</div><div class="btn-row"><button class="btn quiet" type="button" id="addProv">${icon("i-plus")}Add Provider…</button></div></section>` : "";
    const model = r.models && r.models.length ? `<section class="d-sec"><h4>Model</h4><label class="sr-only" for="modelSel">Model</label><select class="select" id="modelSel">${r.models.map((m) => `<option ${m === r.model ? "selected" : ""}>${esc(m)}</option>`).join("")}</select></section>` : "";
    const tools = r.where === "local" && r.id !== "uar" ? "" : `<section class="d-sec"><h4>What it may do</h4><div class="locked">${icon("i-lock")}<span><strong>Text only, no tools.</strong> It can't run commands, read or write files, browse the web or call other tools. Mail is untrusted, so a message can't talk it into doing anything. This can't be turned off here.</span></div></section>`;
    const install = r.status === "missing" ? `<section class="d-sec"><h4>Install</h4><p class="help">Install ${esc(r.name)} with its own installer, then choose Look Again. MailCraft never installs or updates it for you.</p></section>` : "";
    const test = r.status === "ready" || r.signedInAs ? `<section class="d-sec"><h4>Check</h4><div class="btn-row"><button class="btn quiet" type="button" data-test="${r.id}">Test with a Short Question</button></div><p class="test-result" id="testResult" aria-live="polite"></p><p class="help">The test sends one sentence that contains no mail.</p></section>` : "";
    const terms = r.id === "chatgpt" ? `<div class="note info">Your ChatGPT plan's limits apply. OpenAI's terms for Sign in with ChatGPT cover this use.</div>` : r.id === "codex" ? `<div class="note warn">Codex can't be made fully tool-free: even read-only, it can run commands. MailCraft runs it inside the system sandbox, in an empty folder with nothing saved, and stops it the moment it tries a tool. Where no sandbox is available, Codex isn't offered.</div>` : "";
    panel.innerHTML = `
      <div class="detail-head"><span class="rt-icon">${icon(r.icon)}</span><div class="od-field"><h3>${esc(r.name)}</h3><span class="rt-tags">${statusTag(r)}${whereTag(r)}</span></div>
        <button class="icon-btn" type="button" id="closeDetail" aria-label="Close details">${icon("i-close")}</button></div>
      ${facts ? `<section class="d-sec"><h4>Found</h4>${facts}</section>` : ""}
      ${install}
      ${signIn ? `<section class="d-sec"><h4>Sign-in</h4>${signIn}</section>` : ""}
      ${providers}${model}${tools}${terms}${test}`;
  }

  function renderAll() { renderSummary(); renderChain(); renderAccounts(); renderDetail(); }

  /* ---------- events: chain ---------- */
  $("#chain").addEventListener("click", (e) => {
    const mv = e.target.closest("[data-move]");
    if (mv) {
      const i = state.runtimes.findIndex((r) => r.id === mv.dataset.move);
      const j = i + Number(mv.dataset.dir);
      if (j < 0 || j >= state.runtimes.length) return;
      const next = state.runtimes.slice();
      [next[i], next[j]] = [next[j], next[i]];
      state.runtimes = next;
      renderAll();
      const again = document.querySelector(`[data-move="${mv.dataset.move}"][data-dir="${mv.dataset.dir}"]`);
      (again && !again.disabled ? again : document.querySelector(`[data-open="${mv.dataset.move}"]`)).focus();
      return;
    }
    const tg = e.target.closest("[data-toggle]");
    if (tg) {
      const r = byId(tg.dataset.toggle);
      if (!r.on && (r.status === "signedOut" || r.status === "expired")) { openSignIn(r.id, true); return; }
      if (!r.on && r.status === "stopped") { toast(`Start ${r.name} first.`); return; }
      r.on = !r.on;
      renderAll();
      document.querySelector(`[data-toggle="${r.id}"]`).focus();
      if (r.on && leaves(r)) toast(`${r.name} is on. No account allows it yet — choose which below.`);
      return;
    }
    const op = e.target.closest("[data-open]");
    if (op) { state.selected = op.dataset.open; renderAll(); const c = $("#closeDetail"); if (c) c.focus(); return; }
    const si = e.target.closest("[data-signin]");
    if (si) { openSignIn(si.dataset.signin, false); return; }
    const ins = e.target.closest("[data-install]");
    if (ins) { state.selected = ins.dataset.install; renderAll(); return; }
    const st = e.target.closest("[data-start]");
    if (st) {
      const r = byId(st.dataset.start);
      r.status = "checking"; renderAll();
      setTimeout(() => { r.status = "ready"; r.facts = { ...r.facts, "Running": "since just now, 127.0.0.1:7412" }; renderAll(); toast(`${r.name} is running. Turn it on to use it.`); }, 900);
    }
  });

  /* ---------- events: detail ---------- */
  $("#detail").addEventListener("click", (e) => {
    if (e.target.closest("#closeDetail")) { const id = state.selected; state.selected = null; renderAll(); const b = document.querySelector(`[data-open="${id}"]`); if (b) b.focus(); return; }
    const si = e.target.closest("[data-signin]");
    if (si) { openSignIn(si.dataset.signin, false); return; }
    const so = e.target.closest("[data-signout]");
    if (so) {
      const r = byId(so.dataset.signout);
      r.signedInAs = ""; r.status = "signedOut"; r.on = false;
      ACCOUNTS.forEach((a) => { delete state.consent[a.id][r.id]; });
      renderAll(); toast(`Signed out of ${r.name}. Its account permissions were cleared.`); return;
    }
    const te = e.target.closest("[data-test]");
    if (te) {
      const out = $("#testResult"); out.className = "test-result"; out.textContent = "Asking…";
      setTimeout(() => { out.className = "test-result ok"; out.textContent = `Answered in 1.4 s by ${byId(te.dataset.test).name}.`; }, 900);
      return;
    }
    if (e.target.closest("#addProv")) openAdd();
  });
  $("#detail").addEventListener("change", (e) => {
    if (e.target.id === "modelSel") { byId(state.selected).model = e.target.value; toast(`Model set to ${e.target.value}.`); }
  });

  /* ---------- consent ---------- */
  let pendingConsent = null;
  $("#acctTable").addEventListener("click", (e) => {
    const b = e.target.closest("[data-consent]");
    if (!b) return;
    const [acct, rid] = b.dataset.consent.split(":");
    if (state.consent[acct][rid]) {
      delete state.consent[acct][rid];
      renderAll(); toast(`Stopped. No more mail from ${ACCOUNTS.find((a) => a.id === acct).label} goes to ${byId(rid).name}.`);
      const again = document.querySelector(`[data-consent="${acct}:${rid}"]`); if (again) again.focus();
      return;
    }
    pendingConsent = { acct, rid, trigger: b.dataset.consent };
    const a = ACCOUNTS.find((x) => x.id === acct), r = byId(rid);
    $("#consentTitle").textContent = `Let ${r.name} read mail from ${a.addr}?`;
    const to = r.id === "compat" ? "Only the internet providers listed under OpenAI-compatible providers" : `${r.name}, which sends it to ${r.vendor} under your sign-in`;
    $("#consentFacts").innerHTML = `
      <div><dt>What is sent</dt><dd>When you ask, narrow a search or request a draft: the few messages that match, with subject, sender and date. Never attachments, passwords or other accounts.</dd></div>
      <div><dt>To whom</dt><dd>${esc(to)}.</dd></div>
      <div><dt>What it can do</dt><dd>Read the text it is given and answer. No tools.</dd></div>
      <div><dt>How to stop</dt><dd>Select Allowed · Stop in this table. Requests stop at once; what was sent can't be recalled.</dd></div>`;
    $("#consentDlg").showModal();
  });
  function closeConsent(yes) {
    const p = pendingConsent; pendingConsent = null;
    $("#consentDlg").close();
    if (!p) return;
    if (yes) { state.consent[p.acct][p.rid] = true; renderAll(); toast("Allowed for this account."); }
    const b = document.querySelector(`[data-consent="${p.trigger}"]`); if (b) b.focus();
  }
  $("#consentYes").addEventListener("click", () => closeConsent(true));
  $("#consentNo").addEventListener("click", () => closeConsent(false));
  $("#consentDlg").addEventListener("cancel", (e) => { e.preventDefault(); closeConsent(false); });

  /* ---------- sign-in flows ---------- */
  const dlg = $("#signinDlg");
  let signTimer = null, signCtx = null;
  function openSignIn(id, turnOnAfter) {
    const r = byId(id);
    signCtx = { id, turnOnAfter, trigger: document.activeElement };
    if (id === "chatgpt") stepSiwc(r);
    else if (id === "claude" || id === "codex") stepKey(r);
    else stepOwnLogin(r);
    if (!dlg.open) dlg.showModal();
  }
  function setDlg(body, foot) { $("#signinBody").innerHTML = body; $("#signinFoot").innerHTML = foot; const f = dlg.querySelector("[data-focus]") || dlg.querySelector("button"); if (f) f.focus(); }
  function stepSiwc(r) {
    setDlg(`<h2 id="signinTitle" class="dlg-title">Sign in with ChatGPT</h2>
      <p>OpenAI's sign-in page opens in your browser. Sign in and choose Allow; your Go, Plus, Pro or Business plan then pays for MailCraft's answers.</p>
      <div class="locked">${icon("i-lock")}<span>MailCraft keeps the sign-in in the system keychain, never in its files, and only ever uses it for your own questions. You can sign out here or in your ChatGPT settings.</span></div>`,
      `<span class="od-fill"></span><button class="btn quiet" type="button" data-dlg="cancel">Cancel</button><button class="btn primary" type="button" data-dlg="own-start" data-focus>Open Sign-in Page</button>`);
  }
  function stepKey(r) {
    const vendorKey = r.id === "codex" ? "an OpenAI API key for Codex" : "an Anthropic API key for Claude Code";
    const keySrc = r.id === "codex" ? "platform.openai.com" : "console.anthropic.com";
    setDlg(`<h2 id="signinTitle" class="dlg-title">Add ${vendorKey}</h2>
      <div class="od-field form-field"><label for="keyIn">API key <span class="req">required</span></label>
        <input id="keyIn" class="text-input" type="password" autocomplete="off" data-focus aria-describedby="keyHelp">
        <span id="keyHelp" class="help">From ${keySrc}. Kept in the system keychain and handed to ${esc(r.name)} only for MailCraft's own requests.</span></div>
      ${r.id === "claude" ? `<p class="help">Prefer Claude Code's own sign-in? Run Claude Code's Console sign-in, then choose Look Again.</p>` : ""}`,
      `<span class="od-fill"></span><button class="btn quiet" type="button" data-dlg="cancel">Cancel</button><button class="btn primary" type="button" data-dlg="key-save">Save and Test</button>`);
  }
  function stepOwnLogin(r) {
    const what = r.id === "codex" ? "Codex opens OpenAI's sign-in in your browser. Choose Sign in with API key. (A ChatGPT sign-in in Codex is Codex's own; MailCraft won't use it — use ChatGPT plan instead.)" : `${r.name} opens its own sign-in in your browser.`;
    setDlg(`<h2 id="signinTitle" class="dlg-title">Sign in to ${esc(r.name)}</h2>
      <p>${esc(what)}</p>
      <div class="locked">${icon("i-lock")}<span>${esc(r.name)} keeps the sign-in in the system keychain. MailCraft never sees the token; it asks ${esc(r.name)} whether you're signed in.</span></div>`,
      `<span class="od-fill"></span><button class="btn quiet" type="button" data-dlg="cancel">Cancel</button><button class="btn primary" type="button" data-dlg="own-start" data-focus>Open Sign-in in Browser</button>`);
  }
  function stepWaiting(r) {
    setDlg(`<h2 id="signinTitle" class="dlg-title">${r.auth === "apikey" ? "Checking the key with a short test" : "Finish signing in in your browser"}</h2>
      <div class="progress" role="progressbar" aria-label="Waiting for ${esc(r.name)}"><span></span></div>
      <p class="help">Waiting for ${esc(r.name)} to say you're signed in… You can close this; MailCraft checks again when you come back.</p>`,
      `<span class="od-fill"></span><button class="btn quiet" type="button" data-dlg="cancel" data-focus>Cancel</button>`);
  }
  function stepDone(r) {
    r.signedInAs = { chatgpt: "avery@lumen.example (ChatGPT Plus)", claude: "API key ending …7Q2c (Anthropic Console)", codex: "API key (OpenAI Platform)" }[r.id] || "the runtime's own account";
    r.status = "ready";
    if (signCtx && signCtx.turnOnAfter) r.on = true;
    setDlg(`<div class="success">${icon("i-check")}<div class="od-field"><h2 id="signinTitle" class="dlg-title">Signed in to ${esc(r.name)}</h2><span class="help">As ${esc(r.signedInAs)}. ${r.on ? "It is on" : "Turn it on to use it"}; next, choose which accounts may use it.</span></div></div>`,
      `<span class="od-fill"></span><button class="btn primary" type="button" data-dlg="done" data-focus>Done</button>`);
    renderAll();
  }
  function closeSign() {
    clearTimeout(signTimer);
    dlg.close();
    const ctx = signCtx; signCtx = null;
    const target = ctx && (document.querySelector(`[data-toggle="${ctx.id}"]`) || ctx.trigger);
    if (target && target.isConnected) target.focus();
  }
  dlg.addEventListener("click", (e) => {
    const b = e.target.closest("[data-dlg]");
    if (!b || !signCtx) return;
    const r = byId(signCtx.id);
    switch (b.dataset.dlg) {
      case "cancel": closeSign(); toast("Sign-in canceled. Nothing changed."); break;
      case "key-save": {
        const k = $("#keyIn");
        if (!k.value.trim()) { k.setAttribute("aria-invalid", "true"); toast("Paste the key first."); k.focus(); break; }
        stepWaiting(r); signTimer = setTimeout(() => stepDone(r), 1400); break;
      }
      case "own-start": stepWaiting(r); signTimer = setTimeout(() => stepDone(r), 2200); break;
      case "done": closeSign(); break;
      default: break;
    }
  });
  dlg.addEventListener("cancel", (e) => { e.preventDefault(); closeSign(); });

  /* ---------- add provider ---------- */
  const add = $("#addDlg"), aUrl = $("#addUrl"), aErr = $("#addUrlErr"), aSave = $("#addSave");
  function urlKind(v) {
    try { const u = new URL(v); if (/^(localhost|127\.0\.0\.1|\[::1\])$/.test(u.hostname) && u.protocol === "http:") return "local"; if (u.protocol === "https:" && u.hostname.includes(".")) return "net"; return null; } catch { return null; }
  }
  function openAdd() { $("#addName").value = ""; aUrl.value = ""; $("#addKey").value = ""; aErr.hidden = true; aUrl.removeAttribute("aria-invalid"); aSave.disabled = true; add.showModal(); $("#addName").focus(); }
  aUrl.addEventListener("input", () => { aSave.disabled = !urlKind(aUrl.value.trim()); if (urlKind(aUrl.value.trim())) { aErr.hidden = true; aUrl.setAttribute("aria-invalid", "false"); } });
  aUrl.addEventListener("blur", () => {
    const v = aUrl.value.trim(); if (!v) return;
    const ok = !!urlKind(v); aUrl.setAttribute("aria-invalid", String(!ok)); aErr.hidden = ok;
    aErr.textContent = ok ? "" : "Use http://localhost… for a server on this computer, or a full https:// address for one elsewhere. Plain http to another computer is refused.";
  });
  $("#addCancel").addEventListener("click", () => { add.close(); const b = $("#addProv"); if (b) b.focus(); });
  aSave.addEventListener("click", () => {
    const r = byId("compat"), v = aUrl.value.trim(), kind = urlKind(v);
    const name = $("#addName").value.trim() || new URL(v).hostname;
    r.providers = [...r.providers, { name, url: v, where: kind, model: "" }];
    add.close(); renderAll();
    toast(kind === "net" ? `${name} added. It's on the internet, so each account needs your yes.` : `${name} added. It runs on this computer.`);
    const b = $("#addProv"); if (b) b.focus();
  });

  /* ---------- misc ---------- */
  $("#rescan").addEventListener("click", () => {
    const prev = state.runtimes.map((r) => r.status);
    state.runtimes.forEach((r) => { r._prev = r.status; r.status = "checking"; });
    renderAll();
    setTimeout(() => { state.runtimes.forEach((r, i) => { r.status = prev[i]; }); renderAll(); toast("Looked again: 5 of 6 runtimes found. OpenCode isn't installed."); }, 900);
  });
  document.querySelectorAll("input[name=fallback]").forEach((i) => i.addEventListener("change", () => { state.fallback = i.value; toast("Saved."); }));
  document.addEventListener("click", (e) => {
    const t = e.target.closest("[data-toast]"); if (t) toast(t.dataset.toast);
    const th = e.target.closest("[data-theme-set]"); if (th) setTheme(th.dataset.themeSet);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && state.selected && !document.querySelector("dialog[open]")) { const id = state.selected; state.selected = null; renderAll(); const b = document.querySelector(`[data-open="${id}"]`); if (b) b.focus(); }
  });

  setTheme("system");
  renderAll();
})();
