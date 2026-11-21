// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What agents did through the window, for the owner (agent-console, the Agents panel): the last
//! [`MOST_ENTRIES`] requests the gate ([`crate::agent_gate`]) settled, each with when, who the
//! agent said it was, which command, its class, who decided ([`Decided`], one for each way the
//! gate ends a request) and how many things it answered with.
//!
//! It holds ids and counts and nothing else: never a message's text, a subject, an address or a
//! query, and never an agent's params. A command id is kept only when it is one MailCraft knows
//! (classified for agents, or a control method), so an agent can't write words into the log by
//! naming a command that doesn't exist. The reported name is the cleaned, capped text of
//! [`AgentClient`], shown as the agent's own word.
//!
//! It lives in memory only (in `MailApp::asking`, [`crate::agent_gate::Questions`]): it is never
//! written to disk, to `ui.json` or to a log line, and no reply to an agent carries it: the
//! `ui.inspect` serializer doesn't read it, and the commands that show it (`ui.agents`) or turn
//! agent access off (`ui.agentAccess`) are the owner's alone ([`mailcraft_engine::agent`]).

use std::collections::VecDeque;

use mailcraft_engine::agent::Class;
use serde_json::Value;

use crate::control::AgentClient;

/// The most entries kept; the oldest goes when another comes.
pub const MOST_ENTRIES: usize = 200;

/// Who decided about an agent's request: one for each way the gate settles one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decided {
    /// It went ahead: nothing it does needs the owner.
    NoQuestion,
    /// It went ahead without a question: the owner allowed its class in advance
    /// (`mailcraft-cli mcp --allow <class>`).
    AllowedInAdvance,
    /// The owner said yes to the question.
    SaidYes,
    /// The owner said no to the question (the No button, Enter or Escape).
    SaidNo,
    /// Nobody answered the question in [`crate::agent_gate::WAIT_MS`]: a no.
    NoAnswer,
    /// Refused without a question: not for agents, input without `--allow input`, a file named,
    /// what it acts on can't be shown, too many questions waiting, the mailbox it was about gone,
    /// or agent access stopped.
    Refused,
}

impl Decided {
    pub const ALL: [Decided; 6] =
        [Decided::NoQuestion, Decided::AllowedInAdvance, Decided::SaidYes, Decided::SaidNo, Decided::NoAnswer, Decided::Refused];

    /// Whether the gate let the command go ahead (it may still have answered with an error).
    pub fn went_ahead(self) -> bool {
        matches!(self, Decided::NoQuestion | Decided::AllowedInAdvance | Decided::SaidYes)
    }

    /// The words the Agents panel shows for it.
    pub fn label(self) -> &'static str {
        match self {
            Decided::NoQuestion => tl!("Ran: no question needed"),
            Decided::AllowedInAdvance => tl!("Ran: allowed in advance"),
            Decided::SaidYes => tl!("You said yes"),
            Decided::SaidNo => tl!("Not run: you said no"),
            Decided::NoAnswer => tl!("Not run: no answer in 50 s"),
            Decided::Refused => tl!("Refused"),
        }
    }
}

/// One request an agent made, as the owner may see it: ids and counts only.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// When it was settled (`crate::now_ms`).
    pub at_ms: f64,
    /// Who the agent said it was: its own word, cleaned ([`AgentClient`], whose `Debug` never
    /// prints it).
    pub client: Option<AgentClient>,
    /// The command or control method, when it is one MailCraft knows ([`known_command`]).
    pub command: Option<String>,
    /// Its class for agents, when it needs the owner.
    pub class: Option<Class>,
    pub decided: Decided,
    /// It went ahead but answered with an error (or did nothing because what it was asked about
    /// changed).
    pub failed: bool,
    /// How many things it answered with (messages found, ids, rows), when its answer says.
    pub count: Option<u64>,
}

/// The entries, newest last; at most [`MOST_ENTRIES`].
#[derive(Default)]
pub struct Log {
    entries: VecDeque<Entry>,
}

/// Never the entries: a `Debug` of the window must not carry the agents' reported names.
impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log").field("entries", &self.entries.len()).finish()
    }
}

impl Log {
    /// Keeps `entry`, letting the oldest go when there are [`MOST_ENTRIES`] already.
    pub fn record(&mut self, entry: Entry) {
        while self.entries.len() >= MOST_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> + ExactSizeIterator {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The newest entry, if any.
    pub fn last(&self) -> Option<&Entry> {
        self.entries.back()
    }
}

/// The control methods an agent may send besides commands, which the log names as they are.
const METHODS: &[&str] = &[
    "engine.execute",
    "command",
    "engine.commands",
    "mailbox.inspect",
    "ui.inspect",
    "ui.alerts",
    "ui.parity",
    "ui.resize",
    "ui.focus",
    "ui.screenshot",
    "ui.click",
    "ui.move",
    "ui.drag",
    "ui.key",
    "ui.text",
    "app.status",
    "app.quit",
    "auth",
    "ui.minimize",
    "ai.providers",
    "ai.setProvider",
    "ai.setActive",
    "ai.setConsent",
    "ai.removeProvider",
];

/// `id` when MailCraft knows it, as a command classified for agents or a control method; `None`
/// for anything else an agent named, which is never kept: the log holds no words of an agent's.
pub fn known_command(id: &str) -> Option<String> {
    (METHODS.contains(&id) || mailcraft_engine::agent::access(id).is_some()).then(|| id.to_string())
}

/// The arrays an answer names its things in, looked for in this order.
const COUNTED: &[&str] = &["messages", "results", "rows", "ids", "items", "events", "contacts", "tasks", "folders", "accounts"];

/// How many things a command answered with: the length of the answer when it is a list, else of
/// the first of [`COUNTED`] it has, else its `count` or `total`. Only a number is kept.
pub fn count_of(result: &Value) -> Option<u64> {
    let len = |v: &Value| v.as_array().map(|a| u64::try_from(a.len()).unwrap_or(u64::MAX));
    if let Some(n) = len(result) {
        return Some(n);
    }
    let map = result.as_object()?;
    COUNTED.iter().find_map(|k| map.get(*k).and_then(len)).or_else(|| ["count", "total"].iter().find_map(|k| map.get(*k).and_then(Value::as_u64)))
}
