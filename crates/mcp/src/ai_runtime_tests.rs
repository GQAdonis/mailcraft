// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's AI runtime commands (ai-runtime-commands 12.6): MCP names none of them in a tool,
//! lists none of them in `list_commands`, and runs none of them, through `execute` or `batch`,
//! whatever the agent was allowed. The window is never asked.

use mailcraft_engine::agent::{AI_RUNTIME_OWNER_ONLY, Class};
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::tools::{Policy, call_tool, tool_definitions};

/// A stand-in for the window that lists the runtime commands as the window would once they are
/// its commands, and notes every request that would run something.
struct Window {
    ran: Vec<String>,
}

impl Backend for Window {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if method == "engine.commands" {
            let mut all: Vec<Value> = AI_RUNTIME_OWNER_ONLY.iter().map(|id| json!({"id": id, "label": "AI", "location": "File › Options"})).collect();
            all.push(json!({"id": "mail.list", "label": "List", "location": "Mail"}));
            return Ok(Value::Array(all));
        }
        self.ran.push(format!("{method} {params}"));
        Ok(json!({}))
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "a stand-in window".into()
    }
}

fn all_allowed() -> Policy {
    Policy { files: None, allow: Class::ALL.to_vec() }
}

#[test]
fn no_tool_names_a_runtime_command() {
    let defs = tool_definitions().to_string();
    for id in AI_RUNTIME_OWNER_ONLY {
        assert!(!defs.contains(&format!("\"{id}\"")) && !defs.contains(&format!("`{id}`")), "{id} is named by a tool");
    }
}

#[test]
fn list_commands_offers_none_of_them() {
    let mut w = Window { ran: Vec::new() };
    for query in ["", "ai", "runtime", "sign"] {
        let r = call_tool(&mut w, &all_allowed(), "list_commands", &json!({"query": query}));
        assert!(!r.is_error);
        let text = r.content[0]["text"].as_str().unwrap_or("");
        for id in AI_RUNTIME_OWNER_ONLY {
            assert!(!text.contains(&format!("\"{id}\"")), "{id} listed for {query:?}");
        }
    }
    let r = call_tool(&mut w, &all_allowed(), "list_commands", &json!({}));
    assert!(r.content[0]["text"].as_str().unwrap_or("").contains("mail.list"), "the rest is still listed");
}

#[test]
fn none_of_them_runs_through_execute_or_batch() {
    let mut w = Window { ran: Vec::new() };
    let pol = all_allowed();
    for id in AI_RUNTIME_OWNER_ONLY {
        let r = call_tool(&mut w, &pol, "execute", &json!({"command": id, "params": {"runtime": "codex", "value": true}}));
        assert!(r.is_error, "{id}");
        assert_eq!(r.content[0]["text"], format!("`{id}` isn't available to agents"));
    }
    let batch: Vec<Value> = AI_RUNTIME_OWNER_ONLY.iter().map(|id| json!({"command": id, "params": {}})).collect();
    let r = call_tool(&mut w, &pol, "batch", &json!({"commands": batch, "keepGoing": true}));
    assert!(!r.content[0]["text"].as_str().unwrap_or("").contains("\"ok\":true"));
    assert!(w.ran.is_empty(), "the window was asked: {:?}", w.ran);
}
