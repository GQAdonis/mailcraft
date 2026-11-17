/* MailCraft proposal prototype — behaviour. All state lives in `state`; every view renders from it. */
(function () {
  "use strict";
  const D = window.MC_DATA;
  const $ = (sel) => document.querySelector(sel);
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const icon = (id, extra = "") => `<svg class="od-icon" ${extra} aria-hidden="true"><use href="#${id}"/></svg>`;

  const state = {
    folder: "inbox", filter: "all", selected: "m2",
    query: null,             // array of query parts when a search is active
    mode: "search",          // search | ask
    view: "message",         // message | answer | compose
    answer: null, answerTimer: null, answerReturn: false,
    compose: null, suggestTimer: null,
    narrow: null,            // { said, parts }
    ai: { mode: "local", localUrl: "http://localhost:11434", remote: {} },
    theme: "system", motion: "system",
    optPage: "ai", consentFor: null,
    messages: D.messages.map((m) => ({ ...m })),
    agents: { connected: D.agents.connected.map((a) => ({ ...a })), pending: D.agents.pending.map((r) => ({ ...r })), log: D.agents.log.map((l) => ({ ...l })) }
  };

  const update = (patch) => Object.assign(state, patch);
  const msgById = (id) => state.messages.find((m) => m.id === id) || null;
  const acctOf = (m) => (D.folders.find((f) => f.id === m.folder) || {}).acct || "work";
  const acctLabel = (id) => (id === "home" ? "Personal" : "Work");
  const nowTime = () => new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const avatarColor = (key) => {
    let h = 0x811c9dc5;
    for (const ch of key.toLowerCase()) h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193) >>> 0;
    return D.avatarColors[h % D.avatarColors.length];
  };
  const initials = (name) => name.split(/\s+/).map((w) => w[0]).slice(0, 2).join("").toUpperCase();

  /* ---------- toast ---------- */
  let toastTimer = null;
  function toast(text) {
    const t = $("#toast");
    t.textContent = text;
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { t.hidden = true; }, 3200);
  }

  /* ---------- theme + motion ---------- */
  const darkMq = window.matchMedia("(prefers-color-scheme: dark)");
  const motionMq = window.matchMedia("(prefers-reduced-motion: reduce)");
  function applyTheme() {
    const root = document.documentElement;
    root.dataset.theme = state.theme;
    root.classList.toggle("sys-dark", state.theme === "system" && darkMq.matches);
    root.classList.toggle("no-motion", state.motion === "off");
  }
  darkMq.addEventListener("change", applyTheme);
  const motionOff = () => state.motion === "off" || (state.motion === "system" && motionMq.matches);

  /* ---------- query matching (the existing query language) ---------- */
  function matches(m, parts) {
    return parts.every((p) => {
      const [k, v] = p.includes(":") ? [p.slice(0, p.indexOf(":")), p.slice(p.indexOf(":") + 1).toLowerCase()] : ["", p.toLowerCase()];
      if (k === "from") return (m.from + " " + m.addr).toLowerCase().includes(v);
      if (k === "has") return v === "attachment" ? !!m.attach : false;
      if (k === "is") return v === "unread" ? m.unread : v === "flagged" ? m.flagged : false;
      if (k === "after") return m.date >= v;
      const hay = (m.subject + " " + m.from + " " + m.body.join(" ")).toLowerCase();
      return hay.includes(v.replace(/s$/, ""));
    });
  }
  function visibleMessages() {
    let list = state.query ? state.messages.filter((m) => m.folder !== "junk" && matches(m, state.query)) : state.messages.filter((m) => m.folder === state.folder);
    if (state.filter === "unread") list = list.filter((m) => m.unread);
    return list;
  }

  /* ---------- folders ---------- */
  function renderFolders() {
    const html = D.accounts.map((a, i) => {
      const items = D.folders.filter((f) => f.acct === a.id).map((f) => {
        const msgs = state.messages.filter((m) => m.folder === f.id);
        const n = f.quietCount ? msgs.length : msgs.filter((m) => m.unread).length;
        const count = n ? `<span class="count${f.quietCount ? " quiet" : ""}">${n}</span>` : "";
        const cur = !state.query && state.folder === f.id;
        return `<button class="f-item" type="button" data-folder="${f.id}" aria-current="${cur}"><span class="od-truncate">${esc(f.name)}</span>${count}</button>`;
      }).join("");
      return `<div class="f-head${i ? " acct" : ""}">${icon("i-chev-down")}<span class="od-truncate">${esc(a.addr)}</span></div>${items}`;
    }).join("");
    $("#folderList").innerHTML = html;
  }

  /* ---------- list ---------- */
  function rowHtml(m) {
    const sel = m.id === state.selected;
    const icons = [m.attach ? icon("i-clip") : "", m.pinned ? icon("i-pin") : "", m.important ? `<span title="High importance" style="color:var(--danger);font-weight:700">!</span>` : ""].join("");
    return `<div class="row${m.unread ? " is-unread" : ""}${m.flagged ? " is-flagged" : ""}" role="option" tabindex="${sel ? 0 : -1}" aria-selected="${sel}" data-id="${m.id}">
      <span>${m.unread ? icon("i-dot", 'class="od-icon row-dot"').replace('class="od-icon" ', "") : ""}</span>
      <span class="row-main">
        <span class="row-from od-truncate">${esc(m.from)}</span>
        <span class="row-subj od-truncate">${esc(m.subject)}</span>
        <span class="row-prev od-truncate">${esc(m.body.join(" "))}</span>
      </span>
      <span class="row-side">
        <span class="row-time od-nowrap">${esc(m.time)}</span>
        <span class="row-icons">${icons}</span>
        <button class="row-flag${m.flagged ? " is-on" : ""}" type="button" data-flag="${m.id}" aria-pressed="${m.flagged}" aria-label="${m.flagged ? "Clear flag" : "Flag"}: ${esc(m.subject)}">${icon(m.flagged ? "i-flag-fill" : "i-flag")}</button>
      </span>
    </div>`;
  }
  function renderList() {
    const list = visibleMessages();
    const box = $("#listScroll");
    if (!list.length) {
      const folderName = (D.folders.find((f) => f.id === state.folder) || {}).name || "this folder";
      box.innerHTML = state.query
        ? `<div class="empty">${icon("i-search")}<h3>No mail matches</h3><p>Try removing a part of the search, or clear it to see ${esc(folderName)} again.</p><button class="btn quiet" type="button" data-clear>Clear search</button></div>`
        : `<div class="empty">${icon("i-mail")}<h3>Nothing in ${esc(folderName)}</h3><p>${state.filter === "unread" ? "You've read everything here." : "Messages you move here will show up in this list."}</p></div>`;
    } else {
      let html = "", group = null;
      for (const m of list) {
        if (m.group !== group) { group = m.group; html += `<div class="group-label" role="presentation">${esc(group)}</div>`; }
        html += rowHtml(m);
      }
      box.innerHTML = html;
    }
    document.querySelectorAll(".seg-btn").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.filter === state.filter)));
    const as = $("#activeSearch");
    as.hidden = !state.query;
    if (state.query) $("#activePills").innerHTML = state.query.map((p) => `<span class="pill static">${esc(p)}</span>`).join("") + `<span class="help">in all mailboxes · ${list.length} found</span>`;
    const unread = list.filter((m) => m.unread).length;
    $("#statusCount").textContent = `${list.length} items · Unread: ${unread}`;
  }

  /* ---------- reading: message ---------- */
  function messageHtml(m) {
    const color = avatarColor(m.addr);
    const meta = [m.category ? `<span class="chip cat">${esc(m.category)}</span>` : "", m.flagged ? `<span class="chip flag">${icon("i-flag-fill")}${esc(m.due || "Flagged")}</span>` : ""].join("");
    const attach = m.attach ? `<div class="attach">${icon("i-clip")}<span class="od-field"><span class="attach-name">${esc(m.attach.name)}</span><span class="attach-size">${esc(m.attach.size)}</span></span></div>` : "";
    const back = state.answerReturn && state.answer ? `<button class="link-btn" type="button" data-back-answer style="margin:0 0 var(--s3) calc(-1 * var(--s2))">${icon("i-chev-left")} Back to the answer</button>` : "";
    return `<article class="msg"><div class="msg-inner">
      <button class="back-btn" type="button" data-back-list>${icon("i-chev-left")}Messages</button>
      ${back}
      <div class="msg-top">
        <h1 class="msg-subject od-fill">${esc(m.subject)}</h1>
        <div class="msg-actions">
          <button class="icon-btn" type="button" data-action="reply" aria-label="Reply">${icon("i-reply")}</button>
          <button class="icon-btn" type="button" data-action="replyall" aria-label="Reply all">${icon("i-replyall")}</button>
          <button class="icon-btn" type="button" data-action="forward" aria-label="Forward">${icon("i-forward")}</button>
          <button class="icon-btn" type="button" data-action="delete" aria-label="Delete">${icon("i-trash")}</button>
        </div>
      </div>
      <div class="sender">
        <span class="sender-av" style="background:${color}" aria-hidden="true">${esc(initials(m.from))}</span>
        <div class="sender-id">
          <div class="sender-line"><span class="sender-name">${esc(m.from)}</span><span class="sender-addr">${esc(m.addr)}</span></div>
          <span class="sender-to od-truncate">To: ${esc(m.to)}</span>
        </div>
        <span class="msg-date od-nowrap">${esc(m.time)}</span>
      </div>
      ${meta ? `<div class="meta-row">${meta}</div>` : ""}
      ${attach}
      <div class="msg-body">${m.body.map((p) => `<p>${esc(p)}</p>`).join("")}</div>
      ${state.view === "compose" && state.compose ? composeHtml(m) : ""}
    </div></article>`;
  }

  /* ---------- reading: compose with suggested draft ---------- */
  function composeHtml(m) {
    const c = state.compose;
    const kind = c.kind === "forward" ? "Forward" : c.kind === "replyall" ? "Reply all" : "Reply";
    const to = c.kind === "forward" ? "(choose recipients)" : c.kind === "replyall" ? `${m.from}; ${m.to}` : m.from;
    const aiOn = state.ai.mode !== "off";
    const note = c.suggested
      ? `<div class="suggest-note" id="suggestNote">${icon("i-spark")}<span>${c.edited ? "Suggested draft, edited by you." : "Suggested by your model on this computer. Edit it freely; nothing is sent until you choose Send."}</span></div>`
      : "";
    return `<section class="compose${c.suggested && !c.edited ? " has-suggestion" : ""}" aria-label="${kind}">
      <div class="compose-head">${kind} to ${esc(to)}</div>
      <label class="sr-only" for="composeText">Message</label>
      <textarea id="composeText" placeholder="Write your reply">${esc(c.text)}</textarea>
      ${note}
      <div class="compose-foot od-row">
        <button class="btn primary" type="button" data-send ${c.text.trim() ? "" : "disabled"}>${icon("i-send")}Send</button>
        <button class="btn quiet" type="button" data-discard>Discard</button>
        <span class="od-fill"></span>
        ${aiOn ? `<button class="btn quiet" type="button" data-suggest ${c.streaming ? "disabled" : ""}>${icon("i-spark")}${c.streaming ? "Writing…" : c.suggested ? "Suggest again" : "Suggest a draft"}</button>` : `<span class="help">AI is off</span>`}
      </div>
    </section>`;
  }

  /* ---------- reading: answer ---------- */
  function citeOrder(a) {
    const order = [];
    a.parts.join(" ").replace(/\{(m\d+)\}/g, (_, id) => { if (!order.includes(id)) order.push(id); return ""; });
    return order;
  }
  function answerTextHtml(a) {
    const order = citeOrder(a);
    let budget = a.shown;
    const out = [];
    for (const part of a.parts) {
      if (budget <= 0) break;
      const tokens = part.split(/(\{m\d+\}|\s+)/).filter(Boolean);
      let html = "";
      for (const t of tokens) {
        if (budget <= 0) break;
        const cm = t.match(/^\{(m\d+)\}$/);
        if (cm) {
          const n = order.indexOf(cm[1]) + 1;
          const m = msgById(cm[1]);
          html += `<button class="cite" type="button" data-cite="${cm[1]}" aria-label="Source ${n}: ${esc(m ? m.subject : "message")}">${n}</button>`;
        } else html += esc(t);
        budget -= 1;
      }
      out.push({ bullet: part.startsWith("• "), html: html.replace(/^• /, "") });
    }
    let res = "", inList = false;
    for (const o of out) {
      if (o.bullet && !inList) { res += "<ul>"; inList = true; }
      if (!o.bullet && inList) { res += "</ul>"; inList = false; }
      res += o.bullet ? `<li>${o.html}</li>` : `<p>${o.html}</p>`;
    }
    if (inList) res += "</ul>";
    return res;
  }
  function tokenCount(a) { return a.parts.reduce((n, p) => n + p.split(/(\{m\d+\}|\s+)/).filter(Boolean).length, 0); }

  function answerHtml() {
    const a = state.answer;
    const back = `<button class="back-btn" type="button" data-back-list>${icon("i-chev-left")}Messages</button>`;
    if (!a) return "";
    if (a.off) {
      return `<section class="answer"><div class="answer-inner">${back}
        <div class="answer-kicker">Ask your mail</div>
        <div class="answer-q"><h2>${esc(a.q)}</h2></div>
        <div class="notice">${icon("i-shield")}<div class="od-stack" style="--od-gap:8px"><strong>AI is off, so nothing was asked.</strong><span>Turn on a model that runs on this computer to ask questions about your mail. Nothing leaves this computer unless you allow it for an account.</span><span><button class="btn primary" type="button" data-open="options-ai">Set up AI</button></span></div></div>
      </div></section>`;
    }
    const order = citeOrder(a);
    const cited = a.status === "done" ? order : order.filter((id) => answerTextHtml({ ...a }).includes(`data-cite="${id}"`));
    const sources = cited.map((id, i) => {
      const m = msgById(id);
      if (!m) return "";
      return `<button class="source" type="button" data-cite="${id}"><span class="cite" aria-hidden="true">${i + 1}</span><span class="source-main"><span class="source-subj od-truncate">${esc(m.subject)}</span><span class="source-from od-truncate">${esc(m.from)} · ${esc(m.time)}</span></span>${icon("i-chev-left", 'style="transform:rotate(180deg)"')}</button>`;
    }).join("");
    const remoteUrl = state.ai.remote.work;
    const where = remoteUrl
      ? `<span><strong>Answered by your remote model</strong> at ${esc(new URL(remoteUrl).host)}, which you allowed for Work.</span>`
      : `<span><strong>Answered on this computer</strong> by your local model. Nothing was sent anywhere.</span>`;
    const status = a.status === "streaming"
      ? `<div class="answer-status">${icon("i-spark")}<span class="od-fill">Writing the answer from ${a.read} messages…</span><button class="btn quiet" type="button" data-stop>${icon("i-stop")}Stop</button></div>`
      : a.status === "stopped"
        ? `<div class="answer-status">${icon("i-stop")}<span>Stopped. The part written so far is kept.</span></div>`
        : "";
    return `<section class="answer" aria-busy="${a.status === "streaming"}"><div class="answer-inner">
      ${back}
      <div class="answer-kicker">Ask your mail</div>
      <div class="answer-q"><h2>${esc(a.q)}</h2></div>
      ${status}
      <div class="answer-text">${answerTextHtml(a)}${a.status === "streaming" ? '<span class="caret" aria-hidden="true"></span>' : ""}</div>
      ${sources ? `<div class="sources"><h3>Sources</h3>${sources}</div>` : ""}
      ${a.status !== "streaming" ? `<div class="provenance">${where}<span>Read ${a.read} messages from Work's mailboxes. Their text was treated as data, never as instructions.</span></div>
      <div class="suggested">${D.suggestedQuestions.filter((q) => q !== a.q).map((q) => `<button type="button" data-ask="${esc(q)}">${esc(q)}</button>`).join("")}</div>` : ""}
    </div></section>`;
  }

  function renderReading() {
    const box = $("#readingScroll");
    if (state.view === "answer") { box.innerHTML = answerHtml(); return; }
    const m = msgById(state.selected);
    box.innerHTML = m ? messageHtml(m) : `<div class="empty">${icon("i-mail")}<h3>No message selected</h3><p>Choose a message in the list to read it here.</p></div>`;
  }

  /* ---------- AI status + agents ---------- */
  function renderStatus() {
    const s = $("#aiStatus");
    const remotes = Object.keys(state.ai.remote);
    if (state.ai.mode === "off") { s.className = "ai-status off"; s.innerHTML = `${icon("i-shield")}AI off`; }
    else if (remotes.length) { s.className = "ai-status remote"; s.innerHTML = `${icon("i-cloud")}AI: remote allowed for ${remotes.map(acctLabel).join(", ")}`; }
    else { s.className = "ai-status local"; s.innerHTML = `${icon("i-chip")}AI: on this computer`; }
    s.setAttribute("aria-label", s.textContent + ". Open AI settings");
    const n = state.agents.pending.length;
    const b = $("#agentBadge");
    b.hidden = !n;
    b.textContent = String(n);
    $("#agentBtn").setAttribute("aria-label", n ? `Agents, ${n} request${n > 1 ? "s" : ""} waiting` : "Agents");
  }

  function renderAgents() {
    const ag = state.agents;
    const pending = ag.pending.map((r) => {
      const rows = Object.entries(r.preview).filter(([k]) => k !== "body").map(([k, v]) => `<span><span class="k">${esc(k)}:</span> ${esc(v)}</span>`).join("");
      return `<article class="req-card${r.kind === "delete" ? " is-danger" : ""}" aria-label="${esc(r.agent)} ${esc(r.what)}">
        <div class="od-stack" style="--od-gap:2px"><span class="req-who">${esc(r.agent)} ${esc(r.what)}</span><span class="req-what">Connected over MCP. It can't do this without you.</span></div>
        <div class="req-preview">${rows}${r.preview.body ? `<div class="body">${esc(r.preview.body)}</div>` : ""}</div>
        <div class="od-row req-actions">
          <button class="btn ${r.kind === "delete" ? "danger" : "primary"}" type="button" data-allow="${r.id}">${r.kind === "delete" ? "Delete it" : "Send it"}</button>
          <button class="btn quiet" type="button" data-decline="${r.id}">Decline</button>
        </div>
        <span class="req-note">Allows this one request only. The agent is told your answer.</span>
      </article>`;
    }).join("");
    const conns = ag.connected.map((c) => `<div class="conn"><div class="od-stack" style="--od-gap:2px"><span class="conn-name">${esc(c.name)}</span><span class="conn-meta">${esc(c.via)} · can search and read</span></div><button class="btn quiet" type="button" data-disconnect="${c.id}">Disconnect</button></div>`).join("")
      || `<p class="help">No agents are connected. Agents connect through MailCraft's MCP server; see Options › AI and agents.</p>`;
    const log = ag.log.slice().reverse().map((l) => `<div class="log-item"><span class="log-dot ${l.dot}" aria-hidden="true"></span><span>${esc(l.text)}</span><span class="log-time od-nowrap">${esc(l.time)}</span></div>`).join("");
    $("#agentBody").innerHTML = `
      <section class="d-section"><h3>Waiting for you</h3>${pending || `<p class="help">Nothing is waiting. Agents can search and read; sending, deleting and account changes always come here first.</p>`}</section>
      <section class="d-section"><h3>Connected</h3>${conns}</section>
      <section class="d-section"><h3>Activity</h3><div class="log">${log}</div></section>`;
  }

  function renderAll() { renderFolders(); renderList(); renderReading(); renderStatus(); if (!$("#agentDrawer").hidden) renderAgents(); }

  /* ---------- selection ---------- */
  function select(id, opts = {}) {
    const m = msgById(id);
    if (!m) return;
    if (m.unread && !opts.keepUnread) m.unread = false;
    stopSuggest();
    update({ selected: id, view: "message", compose: null, answerReturn: !!opts.fromAnswer });
    if (opts.fromAnswer && !visibleMessages().some((x) => x.id === id)) update({ folder: m.folder, query: null, filter: "all" });
    renderAll();
    $("#workspace").classList.add("show-reading");
    if (opts.focusRow) { const row = document.querySelector(`.row[data-id="${id}"]`); if (row) row.focus(); }
    $("#readingScroll").scrollTop = 0;
  }

  $("#listScroll").addEventListener("click", (e) => {
    const flag = e.target.closest("[data-flag]");
    if (flag) { const m = msgById(flag.dataset.flag); m.flagged = !m.flagged; renderAll(); const again = document.querySelector(`[data-flag="${m.id}"]`); if (again) again.focus(); return; }
    if (e.target.closest("[data-clear]")) { clearSearch(); return; }
    const row = e.target.closest(".row");
    if (row) select(row.dataset.id);
  });
  $("#listScroll").addEventListener("keydown", (e) => {
    const row = e.target.closest(".row");
    if (!row || e.target !== row) return;
    const rows = [...document.querySelectorAll(".row")];
    const i = rows.indexOf(row);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
      if (next) select(next.dataset.id, { focusRow: true });
    } else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(row.dataset.id, { focusRow: true }); }
  });

  $("#folderList").addEventListener("click", (e) => {
    const f = e.target.closest("[data-folder]");
    if (!f) return;
    const first = state.messages.find((m) => m.folder === f.dataset.folder);
    update({ folder: f.dataset.folder, query: null, view: "message", compose: null, selected: first ? first.id : null });
    $("#workspace").classList.remove("folders-open");
    $("#folderToggle").setAttribute("aria-expanded", "false");
    renderAll();
  });
  document.querySelector(".seg").addEventListener("click", (e) => {
    const b = e.target.closest("[data-filter]");
    if (b) { update({ filter: b.dataset.filter }); renderList(); }
  });
  $("#folderToggle").addEventListener("click", () => {
    const open = $("#workspace").classList.toggle("folders-open");
    $("#folderToggle").setAttribute("aria-expanded", String(open));
  });
  $("#railToggle").addEventListener("click", () => {
    const wide = $("#rail").classList.toggle("is-wide");
    $("#railToggle").setAttribute("aria-expanded", String(wide));
    $("#railToggle").setAttribute("aria-label", wide ? "Hide labels" : "Show labels");
  });

  /* ---------- search + narrowing ---------- */
  const input = $("#searchInput");
  const STOP = new Set(["the", "a", "an", "from", "about", "my", "me", "all", "any", "show", "find", "mail", "emails", "email", "messages", "with", "of", "to", "in", "for", "and", "that", "i", "which", "what", "last"]);
  function setMode(mode) {
    update({ mode });
    document.querySelectorAll(".mode-btn").forEach((b) => b.setAttribute("aria-checked", String(b.dataset.mode === mode)));
    $("#search").classList.toggle("is-ask", mode === "ask");
    input.placeholder = mode === "ask" ? "Ask about your mail, e.g. What do I need to prepare for Thursday?" : "Search mail, or describe what you're looking for";
    $("#searchGo").setAttribute("aria-label", mode === "ask" ? "Ask" : "Search");
    closeNarrow();
  }
  document.querySelector(".mode-switch").addEventListener("click", (e) => {
    const b = e.target.closest("[data-mode]");
    if (b) { setMode(b.dataset.mode); input.focus(); }
  });
  function proposeParts(text) {
    const parts = [];
    D.narrowRules.forEach((r) => { if (r.re.test(text) && !parts.includes(r.part)) parts.push(r.part); });
    if (parts.length) return parts;
    return text.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, "").split(/\s+/).filter((w) => w.length > 2 && !STOP.has(w)).slice(0, 4);
  }
  function runQuery(parts) {
    const list = state.messages.filter((m) => m.folder !== "junk" && matches(m, parts));
    update({ query: parts, view: "message", compose: null, selected: list.length ? list[0].id : state.selected });
    renderAll();
  }
  function clearSearch() { update({ query: null }); input.value = ""; renderAll(); }
  function submitSearch() {
    const text = input.value.trim();
    if (!text) return;
    if (state.mode === "ask") { ask(text); return; }
    const looksLikeQuery = /\w+:\S/.test(text) || text.split(/\s+/).length < 2;
    if (looksLikeQuery) { runQuery(text.split(/\s+/)); return; }
    if (state.ai.mode === "off") { runQuery(proposeParts(text)); toast("AI is off, so the words were searched as typed."); return; }
    openNarrow(text, proposeParts(text));
  }
  function openNarrow(said, parts) {
    update({ narrow: { said, parts } });
    renderNarrow();
    $("#narrowPop").hidden = false;
    $("#narrowRun").focus();
  }
  function renderNarrow() {
    const n = state.narrow;
    $("#narrowSaid").textContent = n.said;
    const remote = state.ai.remote.work;
    const where = $("#narrowWhere");
    where.textContent = remote ? `Remote: ${new URL(remote).host}` : "On this computer";
    where.classList.toggle("remote", !!remote);
    $("#narrowPills").innerHTML = n.parts.length
      ? n.parts.map((p, i) => `<span class="pill">${esc(p)}<button class="pill-x" type="button" data-drop="${i}" aria-label="Remove ${esc(p)}">${icon("i-close")}</button></span>`).join("")
      : `<span class="help">Every part was removed. Cancel, or edit the search as text.</span>`;
    $("#narrowRun").disabled = !n.parts.length;
  }
  function closeNarrow(focusInput) {
    $("#narrowPop").hidden = true;
    update({ narrow: null });
    if (focusInput) input.focus();
  }
  $("#narrowPills").addEventListener("click", (e) => {
    const x = e.target.closest("[data-drop]");
    if (!x) return;
    const i = Number(x.dataset.drop);
    update({ narrow: { ...state.narrow, parts: state.narrow.parts.filter((_, j) => j !== i) } });
    renderNarrow();
    const next = document.querySelector("#narrowPills [data-drop]") || $("#narrowEdit");
    next.focus();
  });
  $("#narrowRun").addEventListener("click", () => { const parts = state.narrow.parts; input.value = parts.join(" "); closeNarrow(); runQuery(parts); });
  $("#narrowEdit").addEventListener("click", () => { input.value = state.narrow.parts.join(" "); closeNarrow(true); });
  $("#narrowCancel").addEventListener("click", () => closeNarrow(true));
  $("#searchGo").addEventListener("click", submitSearch);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); submitSearch(); }
    if (e.key === "Escape") { if (state.narrow) closeNarrow(true); else if (state.query) clearSearch(); }
  });
  $("#clearSearch").addEventListener("click", clearSearch);
  document.addEventListener("click", (e) => { if (state.narrow && !e.target.closest(".search-wrap")) closeNarrow(); });

  /* ---------- ask: streamed, cited answer ---------- */
  function ask(q) {
    clearInterval(state.answerTimer);
    if (state.ai.mode === "off") { update({ answer: { q, off: true }, view: "answer" }); renderReading(); $("#workspace").classList.add("show-reading"); return; }
    const lower = q.toLowerCase();
    const script = D.answers.find((a) => a.match.some((w) => lower.includes(w))) || D.answers[D.answers.length - 1];
    const a = { q, parts: script.parts, read: script.read, shown: 0, status: "streaming" };
    a.total = tokenCount(a);
    update({ answer: a, view: "answer", compose: null, answerReturn: false });
    renderReading();
    $("#workspace").classList.add("show-reading");
    const step = motionOff() ? a.total : 2;
    state.answerTimer = setInterval(() => {
      a.shown = Math.min(a.total, a.shown + step);
      if (a.shown >= a.total) { a.status = "done"; clearInterval(state.answerTimer); }
      if (state.view === "answer") renderReading();
    }, 45);
  }
  function stopAnswer() {
    if (!state.answer || state.answer.status !== "streaming") return;
    clearInterval(state.answerTimer);
    state.answer.status = "stopped";
    renderReading();
    const after = document.querySelector(".cite, .suggested button");
    if (after) after.focus();
  }

  /* ---------- compose + suggestion ---------- */
  function stopSuggest() { clearInterval(state.suggestTimer); if (state.compose) state.compose.streaming = false; }
  function openCompose(kind, suggest) {
    const m = msgById(state.selected);
    if (!m) { toast("Choose a message first."); return; }
    stopSuggest();
    update({ view: "compose", compose: { kind, text: "", suggested: false, edited: false, streaming: false } });
    renderReading();
    $("#workspace").classList.add("show-reading");
    if (suggest) suggestDraft(); else { const t = $("#composeText"); if (t) { t.focus(); t.scrollIntoView({ block: "nearest" }); } }
  }
  function suggestDraft() {
    const c = state.compose;
    if (!c) return;
    if (state.ai.mode === "off") { toast("AI is off. Turn it on in AI Settings to get suggestions."); return; }
    const full = D.suggestedReply[state.selected] || D.suggestedReply.fallback;
    const words = full.split(/(\s+)/);
    Object.assign(c, { text: "", suggested: true, edited: false, streaming: true });
    renderReading();
    let i = 0;
    const step = motionOff() ? words.length : 3;
    state.suggestTimer = setInterval(() => {
      i = Math.min(words.length, i + step);
      c.text = words.slice(0, i).join("");
      const t = $("#composeText");
      if (t) t.value = c.text;
      if (i >= words.length) { stopSuggest(); renderReading(); const tt = $("#composeText"); if (tt) tt.focus(); }
    }, 40);
  }
  $("#readingScroll").addEventListener("input", (e) => {
    if (e.target.id !== "composeText" || !state.compose) return;
    const c = state.compose;
    c.text = e.target.value;
    const send = document.querySelector("[data-send]");
    if (send) send.disabled = !c.text.trim();
    if (c.suggested && !c.edited && !c.streaming) {
      c.edited = true;
      e.target.closest(".compose").classList.remove("has-suggestion");
      const note = $("#suggestNote");
      if (note) note.querySelector("span").textContent = "Suggested draft, edited by you.";
    }
  });

  /* ---------- reading-pane + ribbon actions ---------- */
  function removeSelected(verb) {
    const list = visibleMessages();
    const i = list.findIndex((m) => m.id === state.selected);
    if (i < 0) return;
    const m = list[i];
    state.messages = state.messages.filter((x) => x.id !== m.id);
    const next = list[i + 1] || list[i - 1];
    update({ selected: next ? next.id : null, view: "message", compose: null });
    renderAll();
    toast(`${verb}: “${m.subject}”.`);
  }
  function doAction(a) {
    const m = msgById(state.selected);
    switch (a) {
      case "new": toast("New Email opens in its own window, as it does today."); break;
      case "delete": removeSelected("Moved to Deleted Items"); break;
      case "archive": removeSelected("Archived"); break;
      case "reply": case "replyall": case "forward": openCompose(a, false); break;
      case "suggest": openCompose("reply", true); break;
      case "flag": if (m) { m.flagged = !m.flagged; renderAll(); } break;
      case "read": if (m) { m.unread = !m.unread; renderAll(); } break;
      case "pin":
        if (m) {
          if (!m.pinned) { m.prevGroup = m.group; m.group = "Pinned"; } else { m.group = m.prevGroup || "Earlier"; }
          m.pinned = !m.pinned;
          renderAll();
          toast(m.pinned ? "Pinned to the top of the list." : "Unpinned.");
        }
        break;
      case "ask": setMode("ask"); input.focus(); break;
      default: break;
    }
  }
  document.addEventListener("click", (e) => {
    const act = e.target.closest("[data-action]");
    if (act) { doAction(act.dataset.action); return; }
    const t = e.target.closest("[data-toast]");
    if (t) toast(t.dataset.toast);
    const open = e.target.closest("[data-open]");
    if (open) openOptions(open.dataset.open === "options-ai" ? "ai" : "general");
  });
  $("#readingScroll").addEventListener("click", (e) => {
    const cite = e.target.closest("[data-cite]");
    if (cite) { select(cite.dataset.cite, { fromAnswer: true }); return; }
    if (e.target.closest("[data-back-answer]")) { update({ view: "answer", answerReturn: false }); renderReading(); return; }
    if (e.target.closest("[data-back-list]")) { $("#workspace").classList.remove("show-reading"); return; }
    if (e.target.closest("[data-stop]")) { stopAnswer(); return; }
    const q = e.target.closest("[data-ask]");
    if (q) { input.value = q.dataset.ask; setMode("ask"); ask(q.dataset.ask); return; }
    if (e.target.closest("[data-suggest]")) { suggestDraft(); return; }
    if (e.target.closest("[data-discard]")) { stopSuggest(); update({ view: "message", compose: null }); renderReading(); toast("Draft discarded."); return; }
    if (e.target.closest("[data-send]")) {
      const m = msgById(state.selected);
      stopSuggest();
      update({ view: "message", compose: null });
      renderReading();
      toast(`Sent to ${m ? m.from : "recipient"}. You sent it; no agent did.`);
    }
  });

  /* ---------- help menu ---------- */
  const helpBtn = $("#helpBtn"), helpMenu = $("#helpMenu");
  function setHelp(open) { helpMenu.hidden = !open; helpBtn.setAttribute("aria-expanded", String(open)); if (open) helpMenu.querySelector("button").focus(); }
  helpBtn.addEventListener("click", (e) => { e.stopPropagation(); setHelp(helpMenu.hidden); });
  helpMenu.addEventListener("click", () => setHelp(false));
  document.addEventListener("click", (e) => { if (!helpMenu.hidden && !e.target.closest(".help-wrap")) setHelp(false); });

  /* ---------- agents drawer ---------- */
  const drawer = $("#agentDrawer"), agentBtn = $("#agentBtn");
  function setDrawer(open) {
    drawer.hidden = !open;
    agentBtn.setAttribute("aria-expanded", String(open));
    if (open) { renderAgents(); $("#agentClose").focus(); } else agentBtn.focus();
  }
  agentBtn.addEventListener("click", () => setDrawer(drawer.hidden));
  $("#agentClose").addEventListener("click", () => setDrawer(false));
  $("#agentBody").addEventListener("click", (e) => {
    const allow = e.target.closest("[data-allow]"), decline = e.target.closest("[data-decline]"), disc = e.target.closest("[data-disconnect]");
    const ag = state.agents;
    if (allow || decline) {
      const id = (allow || decline).dataset[allow ? "allow" : "decline"];
      const r = ag.pending.find((x) => x.id === id);
      ag.pending = ag.pending.filter((x) => x.id !== id);
      if (allow) {
        if (r.kind === "delete") state.messages = state.messages.filter((m) => !(r.targets || []).includes(m.id));
        if (r.kind === "send") state.messages = state.messages.filter((m) => m.id !== "m9");
        ag.log.push({ dot: "ok", text: `You allowed ${r.agent} to ${r.kind === "send" ? "send the reply to Lin Zhao" : "delete 1 message"}`, time: nowTime() });
        toast(r.kind === "send" ? "Sent to Lin Zhao, with your approval." : "Deleted, with your approval.");
      } else {
        ag.log.push({ dot: "no", text: `You declined ${r.agent}'s request; nothing was ${r.kind === "send" ? "sent" : "deleted"}`, time: nowTime() });
        toast(`Declined. ${r.agent} was told no.`);
      }
      renderAll();
      const nextBtn = drawer.querySelector("[data-allow]") || $("#agentClose");
      nextBtn.focus();
    }
    if (disc) {
      const c = ag.connected.find((x) => x.id === disc.dataset.disconnect);
      ag.connected = ag.connected.filter((x) => x !== c);
      ag.pending = ag.pending.filter((r) => r.agent !== c.name);
      ag.log.push({ dot: "no", text: `You disconnected ${c.name}`, time: nowTime() });
      renderAll();
      (drawer.querySelector("[data-disconnect]") || $("#agentClose")).focus();
    }
  });

  /* ---------- options ---------- */
  const optDlg = $("#optionsDlg");
  let optReturn = null;
  function openOptions(page) {
    optReturn = document.activeElement;
    update({ optPage: page });
    renderOptions();
    if (!optDlg.open) optDlg.showModal();
    const cur = optDlg.querySelector(`.opt-link[data-page="${page}"]`);
    if (cur) cur.focus();
  }
  function segChoice(name, val, opts) {
    return `<div class="seg-choice" role="group" aria-label="${name}">${opts.map(([v, l]) => `<button type="button" data-set="${name}" data-val="${v}" aria-pressed="${v === val}">${l}</button>`).join("")}</div>`;
  }
  function renderOptions() {
    optDlg.querySelectorAll(".opt-link").forEach((l) => l.setAttribute("aria-current", String(l.dataset.page === state.optPage)));
    const page = $("#optPage");
    if (state.optPage === "general") {
      page.innerHTML = `<h3>General</h3>
        <div class="opt-section"><h4>Theme</h4>${segChoice("theme", state.theme, [["system", "Use system setting"], ["light", "Light"], ["dark", "Dark"]])}</div>
        <div class="opt-section"><h4>Animations</h4>${segChoice("motion", state.motion, [["system", "Use system setting"], ["on", "On"], ["off", "Off"]])}<p class="help">Off stops animations. A spinner that shows work in progress keeps turning.</p></div>`;
      return;
    }
    const accts = D.accounts.map((a) => {
      const url = state.ai.remote[a.id];
      return `<div class="acct-row"><div class="od-stack" style="--od-gap:2px"><span class="acct-name">${esc(a.addr)}</span>
        <span class="acct-state${url ? " on" : ""}">${url ? `Remote model allowed: ${esc(new URL(url).host)}` : "Remote models not allowed. Mail stays on this computer."}</span></div>
        ${url ? `<button class="btn danger" type="button" data-revoke="${a.id}">Revoke</button>` : `<button class="btn quiet" type="button" data-consent="${a.id}" ${state.ai.mode === "off" ? "disabled" : ""}>Allow…</button>`}</div>`;
    }).join("");
    page.innerHTML = `<h3>AI and agents</h3>
      <p class="opt-lead">MailCraft uses a model only when you ask, narrow a search or request a draft. By default it runs on this computer, and mail never leaves it.</p>
      <div class="opt-section"><h4>Where AI runs</h4>
        <label class="choice"><input type="radio" name="aimode" value="off" ${state.ai.mode === "off" ? "checked" : ""}><span class="od-field"><span class="choice-title">Off</span><span class="choice-help">No questions, narrowing or drafts. Search works as it does today.</span></span></label>
        <label class="choice"><input type="radio" name="aimode" value="local" ${state.ai.mode === "local" ? "checked" : ""}><span class="od-field"><span class="choice-title">On this computer</span><span class="choice-help">A model you run yourself, reached only on this computer's own address.</span></span></label>
        <div class="od-field form-field" ${state.ai.mode === "off" ? "hidden" : ""}><label for="localUrl">Local model address</label><input id="localUrl" class="text-input" type="url" value="${esc(state.ai.localUrl)}"><span class="help">Only localhost or 127.0.0.1 addresses are accepted here.</span><span class="field-err" id="localErr" hidden></span></div>
      </div>
      <div class="opt-section"><h4>Remote models, account by account</h4><p class="help">Each account needs its own yes before any of its mail goes to a remote model.</p>${accts}</div>
      <div class="opt-section"><h4>Agents</h4>
        <label class="check od-row"><input type="checkbox" checked disabled><span>Ask me before an agent sends, deletes or changes an account. <span class="help">Always on: an agent can't answer for you.</span></span></label>
        <p class="help">${state.agents.connected.length} agent${state.agents.connected.length === 1 ? "" : "s"} connected over MCP. Manage them from Agents in the title bar.</p>
      </div>`;
  }
  optDlg.addEventListener("click", (e) => {
    const link = e.target.closest(".opt-link");
    if (link) { update({ optPage: link.dataset.page }); renderOptions(); optDlg.querySelector(`.opt-link[data-page="${link.dataset.page}"]`).focus(); return; }
    const set = e.target.closest("[data-set]");
    if (set) { update({ [set.dataset.set]: set.dataset.val }); applyTheme(); renderOptions(); optDlg.querySelector(`[data-set="${set.dataset.set}"][data-val="${set.dataset.val}"]`).focus(); return; }
    const c = e.target.closest("[data-consent]");
    if (c) { openConsent(c.dataset.consent); return; }
    const r = e.target.closest("[data-revoke]");
    if (r) {
      const { [r.dataset.revoke]: _gone, ...rest } = state.ai.remote;
      state.ai = { ...state.ai, remote: rest };
      renderOptions(); renderStatus();
      toast(`Revoked. No more mail from ${acctLabel(r.dataset.revoke)} goes to a remote model.`);
      const b = optDlg.querySelector(`[data-consent="${r.dataset.revoke}"]`); if (b) b.focus();
    }
  });
  optDlg.addEventListener("change", (e) => {
    if (e.target.name === "aimode") {
      state.ai = { ...state.ai, mode: e.target.value, remote: e.target.value === "off" ? {} : state.ai.remote };
      renderOptions(); renderStatus();
      const r = optDlg.querySelector(`input[name="aimode"][value="${e.target.value}"]`); if (r) r.focus();
    }
  });
  optDlg.addEventListener("focusout", (e) => {
    if (e.target.id !== "localUrl") return;
    const ok = /^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?\/?$/.test(e.target.value.trim());
    const err = $("#localErr");
    e.target.setAttribute("aria-invalid", String(!ok));
    if (err) { err.hidden = ok; err.textContent = ok ? "" : "Use an address on this computer, such as http://localhost:11434. Remote models are allowed per account below."; }
    if (ok) state.ai = { ...state.ai, localUrl: e.target.value.trim() };
  });
  $("#optDone").addEventListener("click", () => optDlg.close());
  optDlg.addEventListener("close", () => { if (optReturn && optReturn.isConnected) optReturn.focus(); else $("#aiStatus").focus(); });

  /* ---------- consent ---------- */
  const consentDlg = $("#consentDlg"), urlIn = $("#providerUrl"), urlErr = $("#providerErr"), check = $("#consentCheck"), allowBtn = $("#consentAllow");
  const validUrl = (v) => { try { const u = new URL(v); return u.protocol === "https:" && u.hostname.includes("."); } catch { return false; } };
  function syncConsent() { allowBtn.disabled = !(check.checked && validUrl(urlIn.value.trim())); }
  function openConsent(acct) {
    update({ consentFor: acct });
    $("#consentAcct").textContent = D.accounts.find((a) => a.id === acct).addr;
    urlIn.value = ""; check.checked = false; urlErr.hidden = true; urlErr.textContent = ""; urlIn.removeAttribute("aria-invalid");
    syncConsent();
    consentDlg.showModal();
    urlIn.focus();
  }
  urlIn.addEventListener("blur", () => {
    const v = urlIn.value.trim();
    if (!v) return;
    const ok = validUrl(v);
    urlIn.setAttribute("aria-invalid", String(!ok));
    urlErr.hidden = ok;
    urlErr.textContent = ok ? "" : "Enter a full address that starts with https://, for example https://models.example.com/v1. Plain http is refused for remote models.";
  });
  urlIn.addEventListener("input", () => { if (validUrl(urlIn.value.trim())) { urlErr.hidden = true; urlIn.setAttribute("aria-invalid", "false"); } syncConsent(); });
  check.addEventListener("change", syncConsent);
  $("#consentCancel").addEventListener("click", () => consentDlg.close());
  allowBtn.addEventListener("click", () => {
    const acct = state.consentFor;
    state.ai = { ...state.ai, remote: { ...state.ai.remote, [acct]: urlIn.value.trim() } };
    consentDlg.close();
    renderOptions(); renderStatus();
    toast(`Allowed for ${acctLabel(acct)}. The status bar now shows it.`);
    const b = optDlg.querySelector(`[data-revoke="${acct}"]`); if (b) b.focus();
  });
  consentDlg.addEventListener("close", () => { const b = optDlg.querySelector(`[data-consent="${state.consentFor}"], [data-revoke="${state.consentFor}"]`); if (b && optDlg.open) b.focus(); });

  /* ---------- global keys ---------- */
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (!helpMenu.hidden) { setHelp(false); helpBtn.focus(); }
      else if (!drawer.hidden && !optDlg.open) setDrawer(false);
      else if (state.view === "answer" && state.answer && state.answer.status === "streaming") stopAnswer();
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setMode("ask"); input.focus(); }
  });

  /* ---------- start ---------- */
  applyTheme();
  setMode("search");
  renderAll();
})();
