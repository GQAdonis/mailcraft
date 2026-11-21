// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Who the MCP client says it is: `clientInfo {name, version}` from `initialize`, which the
//! bridge passes on to the window as `agent.client` (`docs/control-protocol.md`, Agents).
//!
//! It is the client's own word, never checked, and may be anything: only strings are kept, each
//! cleaned by the one list of names (`mailcraft_names::incoming`: one line, no control, format or
//! bidi characters) and cut to [`MAX_CHARS`] characters, so what is sent stays small. The window
//! cleans it again (it trusts no request), shows it as reported, and decides nothing by it. It is
//! never written to a log line.

use serde_json::{Map, Value};

/// The most characters of a reported name or version that are passed on (the window's own
/// bound: `mailcraft_ui_egui::control::MAX_CLIENT_CHARS`).
pub const MAX_CHARS: usize = 64;

/// What an MCP client reported about itself in `initialize`, cleaned.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

/// Never the name itself, so that no `Debug` carries the client's text into a log line.
impl std::fmt::Debug for ClientInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientInfo").field("name", &self.name.as_ref().map(|_| "…")).field("version", &self.version.as_ref().map(|_| "…")).finish()
    }
}

fn field(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).and_then(|s| mailcraft_names::incoming(s, MAX_CHARS))
}

impl ClientInfo {
    /// The `clientInfo` of `initialize`'s params, cleaned; `None` when there is none, it is not an
    /// object, or nothing of it is left.
    pub fn from_initialize(params: &Value) -> Option<Self> {
        let c = params.get("clientInfo").filter(|c| c.is_object())?;
        let info = Self { name: field(c.get("name")), version: field(c.get("version")) };
        (info.name.is_some() || info.version.is_some()).then_some(info)
    }

    /// As the `client` of a request's `agent` field: the fields there are, and no others.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        if let Some(n) = &self.name {
            m.insert("name".to_string(), Value::String(n.clone()));
        }
        if let Some(v) = &self.version {
            m.insert("version".to_string(), Value::String(v.clone()));
        }
        Value::Object(m)
    }
}
