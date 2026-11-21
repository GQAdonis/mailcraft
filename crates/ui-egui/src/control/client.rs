// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Who an agent says it is: the `client` of a request's `agent` field (`{"agent": {"allow": [...],
//! "client": {"name", "version"}}}`), which the MCP bridge copies from the `clientInfo` its client
//! sent with `initialize` (`docs/control-protocol.md`, Agents).
//!
//! Anything that reads the control key can write any name here, so it is self-reported text and
//! hostile: each field is cleaned by the one list of names (`mailcraft_names::incoming`: one line,
//! no control, format or bidi characters, nothing that shows nothing), cut to
//! [`MAX_CLIENT_CHARS`] characters at a character's boundary, and dropped when nothing is left.
//! It is shown as "reported as ‹name›", or "Unnamed agent", and is never used to decide
//! anything: the gate reads only the `allow` list. It is never written to a log line, and no
//! reply to an agent carries it.

use serde_json::Value;

/// The most characters of a reported name or version that are kept.
pub const MAX_CLIENT_CHARS: usize = 64;

/// What an agent reported about itself, cleaned. Both fields may be missing.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AgentClient {
    pub name: Option<String>,
    pub version: Option<String>,
}

/// Never the name itself: a `Debug` of a request must not carry hostile text into a log line.
impl std::fmt::Debug for AgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentClient").field("name", &self.name.as_ref().map(|_| "…")).field("version", &self.version.as_ref().map(|_| "…")).finish()
    }
}

/// One reported field, cleaned: a string only (a number, an object or anything else is dropped),
/// at most [`MAX_CLIENT_CHARS`] characters, `None` when nothing is left.
fn field(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).and_then(|s| mailcraft_names::incoming(s, MAX_CLIENT_CHARS))
}

/// The `client` of a request's `agent` field, cleaned; `None` when there is none, it is not an
/// object, or neither field has anything left once cleaned.
pub fn agent_client(agent: &Value) -> Option<AgentClient> {
    let c = agent.get("client").filter(|c| c.is_object())?;
    let client = AgentClient { name: field(c.get("name")), version: field(c.get("version")) };
    (client.name.is_some() || client.version.is_some()).then_some(client)
}

impl AgentClient {
    /// How the window names an agent: "reported as ‹name›", or "Unnamed agent" when it reported
    /// none. The name is the agent's own word, never checked.
    pub fn shown(client: Option<&Self>) -> String {
        match client.and_then(|c| c.name.as_deref()) {
            Some(name) => crate::i18n::fmt(tl!("reported as ‹{name}›"), &[("name", name)]),
            None => tl!("Unnamed agent").to_string(),
        }
    }
}
