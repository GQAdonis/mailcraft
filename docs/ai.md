# AI assistance: model servers, what is sent, and what happens on failure

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** major · **Target:** Microsoft Outlook (Microsoft 365)

MailCraft has no AI service of its own. Searching in words, answering questions from your mail and narrowing the message list work only with a model server you name in **File › Options › AI assistance**. Each one speaks the OpenAI-style chat API. This page says what each kind of server is sent and what MailCraft does when one fails. [PRIVACY.md](../PRIVACY.md) is the authority on privacy; this page explains the mechanics.

## Kinds of model server

| Kind | Where it runs | Connection | Before mail is sent |
|---|---|---|---|
| **On this computer** | A program on this computer (apfel serving Apple's on-device model, Ollama, LM Studio, …) | Loopback only (`127.0.0.1`, `::1`, `localhost`); may be plain HTTP | Nothing more is asked, but its model list is checked first (below) |
| **Gateway on this computer** | A local proxy that forwards to the internet (LiteLLM, liter-llm, …) | Loopback, like the row above | Your yes for each account, as for an internet service |
| **Internet service** | A provider on the internet or your network | HTTPS only; certificate pinning available | Your yes for each account |

A key is kept only in the operating system's credential store, and only for a gateway or internet service. A model on this computer has no key field.

### Is a "local" server really local?

A program on loopback can pass everything on to the internet. Before MailCraft first sends a server set up as "on this computer" anything in a session, it reads that server's `GET /v1/models` list, with no mail and no key:

- **Refused:** model names or `owned_by` values of known hosted providers (`anthropic`, `azure`, `bedrock`, `vertex_ai`, `gemini`, `groq`, `openrouter`, `dashscope`, `…-token`). Set such a server up as a gateway.
- **Asked once:** any other publisher prefix (`openai/gpt-oss-20b`, `lmstudio-community/…`, `hf.co/…`), several different `owned_by` values, an `owned_by` of `openai` (LiteLLM's default), or names of hosted model families (`gpt-…`, `claude-…`, `gemini-…`, `qwen4-max`, …). Asked "Does this server send your mail to the internet?", with the answers *Yes, It Does* (it becomes a gateway), *No, It Doesn't* (remembered for this address and this model list) and *Not Now* (nothing is sent; you're asked again next time).
- **Passed:** plain local names with no publisher prefix and no hosted model family (`llama3.2:latest`, `mistral:7b`).

A list MailCraft can't read (an error, or not a model list) is not checked; a server that hides what it forwards to that way, or by listing plain names, looks local. Setting it up as a gateway is then up to you.

## Primary and backup

You choose a **primary** model server (or None) and a **backup**: Apple's on-device model, or None.

- **Never set up:** MailCraft uses the on-device model, once it is found and you have confirmed it.
- **Primary None:** nothing is sent anywhere, and the backup is off too.
- **When the primary fails** before any of its answer has arrived, and the failure is one a second model can help with (unreachable, refused, timed out before the first byte, credential store unavailable), the backup answers. The answer says which model gave it, and that the primary didn't answer.
- **No fallback** once part of an answer is on screen. Because the backup is always on this computer, it may also answer when the primary is refused for its settings or has no consent for the account, and it never sends mail anywhere else.

### Finding the on-device model

MailCraft looks for the on-device model only when File › Options opens or when you ask your first question, and never while the backup is None. It asks apfel's usual address (`127.0.0.1:11434`) and any ports you add for `/v1/models` and `/health`. These are loopback requests with no mail and no key.

A server is the on-device model when its list is exactly `apple-foundationmodel` and its health answer has apfel's shape. Candidates are listed with a **Use It** button. Nothing is sent to an address until you press Use It for it. The identity is checked again at first use in each session and after any error (not before every request); if it changed, MailCraft stops using it and asks again.

## Context window

Each server's context window is read from its `/health` or `/v1/models` entry (`context_length`, `context_window`, `max_model_len`), clamped to 256–1,048,576 tokens. You can also type it in Options. The defaults are 4,096 on this computer and 32,768 for a gateway or internet service.

The prompt is built for the window of the model that answers. When the backup takes over, MailCraft rebuilds the prompt for the backup's window from the same passages. That can drop or renumber passages, and citations follow the rebuilt numbering.

## Structured answers

Answers ask for `{"answer": "...", "citations": [1, 2]}`, with at most three citations. They are streamed: the answer text appears as it arrives, and the citations are read at the end.

If a server rejects the requested format (HTTP 400), MailCraft steps down on the same server: JSON schema, then JSON object, then plain text, reading `[n]`, "message 2" and "(2)" for citations. It remembers the level that worked for the session.

An answer that names no message is shown as "No message was named as the source". One whose citations can't be read keeps its text, marked "MailCraft couldn't read which messages this came from".

## Narrowing the list in words

Narrowing sends your words, a description of MailCraft's query language with a few worked examples, and today's date: never any mail. The model is asked for `{"query": "..."}`.

What your words say plainly is read by fixed rules on this computer and wins over the model's reading: a sender (`from Ada Abernathy`, `Greta sent me`, `Dmitri's emails`, `not from Tom`), a recipient (`sent to Bob`), dates (`since 21 June`, `last Tuesday`, `in March`, `between 1 May and 3 June`, `last week`), attachments (`with an attachment`, `with a PDF`, `without attachments`), and `unread` and `flagged`. Words that only describe the asking (`mail`, `anything`, `notes that mention`, `sent me`) are dropped from the model's search. A name must be written with a capital (or be an address); a month, a day of the week or a date with no word before it saying how it bounds the mail is left to the model. When nothing of the model's search is left, or it gives no search MailCraft can read twice, your own remaining words are the search. Whatever is offered is checked against the query language, and nothing changes in the list until you accept it.

## When something fails

Every failure has a kind: `not_configured`, `no_consent`, `unreachable`, `refused` (with the HTTP status), `timed_out` (before or after the first byte), `certificate`, `settings`, `credential_store`, `malformed`, `too_large`, `cancelled`. The window's message comes from the kind, never from the server's wording. What the server said is kept under **Details** as text, never followed or run.

A credential store that doesn't answer within 5 seconds fails the request as `credential_store` instead of freezing it.

## Runtimes beyond model servers

Besides the model servers above, the desktop app can take its answers from other runtimes, each off until you turn it on, tried in the order you set:

| Runtime | Where text goes | Under whose account | What MailCraft holds |
|---|---|---|---|
| **Apple's on-device model**, a model server on this computer | Stays on this computer | — | Nothing secret |
| **A gateway, an internet service** | To that provider | The key's account | The key, in the credential store |
| **ChatGPT plan** (Sign in with ChatGPT) | OpenAI's Responses API | Your ChatGPT plan | The issued client id and refresh token in the credential store; the access token in memory |
| **Claude Code** (the `claude` program you installed) | Anthropic | Your own Claude login and plan quota | Nothing: MailCraft never reads the program's login |
| **Codex** (the `codex` program you installed) | OpenAI | Your own ChatGPT login and plan quota | Nothing: MailCraft never reads your Codex folder's login |
| **Universal Agent Runtime** you started yourself | Wherever it is set up to send | Its own | Nothing |

Every runtime that leaves this computer needs your yes for each account before that account's mail goes to it, and none needs an API key for Claude or Codex: MailCraft bills nothing by the token. The CLIs are used as text models only: their tools are turned off, and a run that tries to use a tool anyway is stopped with every process it started, its text thrown away, and you are told before another runtime is tried. When one fails before any answer arrives, the next one is tried, you are asked first, or the failure is shown, as you chose. Codex runs only under the operating system's sandbox, so it isn't offered on Windows.

[PRIVACY.md](../PRIVACY.md#ai-runtimes) says what each runtime is sent, where its sign-in lives and how to stop it, including what text of your own Codex skills may reach OpenAI; the [threat model](threat-model.md#local-coding-agents-as-text-models) says how the CLIs are contained and what risk is left.

## Agents

External agents (MCP, the control channel) may search mail read-only. They can't ask the model, change model servers, answer the gateway question, run Test Connection or confirm the on-device model. Nor can they list, order, turn on, sign in to, sign out of or give consent for a runtime (the `ai.*` runtime commands are `never` for agents). Those are the owner's alone.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | major | New: model server kinds, primary and backup, finding Apple's on-device model, context windows, structured answers and their step-down, failure kinds, agents |
