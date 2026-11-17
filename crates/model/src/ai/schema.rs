// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The JSON schemas MailCraft sends a model for a structured answer, and the portable subset of
//! JSON Schema they keep to.
//!
//! Providers take different parts of JSON Schema: a provider on this computer refuses
//! `maxLength` with HTTP 400, others refuse `pattern` or `format`. What every provider MailCraft
//! speaks to takes is their intersection: `type` (one name, never a list), `properties`,
//! `required`, `enum` of strings, numeric `minimum` and `maximum`, `items`, `minItems` and
//! `maxItems`, and `additionalProperties: false`. Nothing else, so a limit a schema can't say
//! (a length, a pattern) is held in code where the answer is read (`MAX_PROPOSAL_CHARS`).
//!
//! [`PRODUCT_SCHEMAS`] lists every schema the product sends, and a test holds each to the subset
//! ([`portable_problems`]), so that a keyword outside it can't slip back in. Adding a keyword to
//! the subset is a decision, made on evidence that every provider takes it.

use serde_json::Value;

use super::answer::ANSWER_SCHEMA;
use super::narrow::NARROW_SCHEMA;

/// Every JSON schema MailCraft sends a model, by what it is for. (An answer's schema is made for
/// its number of messages, `answer_schema`; [`ANSWER_SCHEMA`] is it for the most there can be,
/// and a test holds every other number to the subset too.)
pub const PRODUCT_SCHEMAS: &[(&str, &str)] = &[("narrow", NARROW_SCHEMA), ("answer", ANSWER_SCHEMA)];

/// How deep a schema may nest.
const MAX_DEPTH: usize = 16;
/// Most problems reported for one schema.
const MAX_PROBLEMS: usize = 64;
/// Most schema nodes looked at.
const MAX_NODES: usize = 4_096;

/// The type names of the subset.
const TYPES: &[&str] = &["object", "string", "number", "integer", "boolean", "array"];

/// What is wrong with `schema` (JSON text) for the portable subset, each in words naming where
/// (a JSON pointer); empty when it keeps to it. Bounded: at most 16 levels, 4,096 nodes and 64
/// problems are looked at or said.
pub fn portable_problems(schema: &str) -> Vec<String> {
    let value: Value = match serde_json::from_str(schema) {
        Ok(v) => v,
        Err(_) => return vec!["the schema isn't JSON".into()],
    };
    let mut lint = Lint { problems: Vec::new(), nodes: 0 };
    if value.get("type").and_then(Value::as_str) != Some("object") {
        lint.say("#: the root is to be an object (`\"type\": \"object\"`)".into());
    }
    lint.node(&value, "#", 0);
    lint.problems
}

struct Lint {
    problems: Vec<String>,
    nodes: usize,
}

impl Lint {
    fn say(&mut self, problem: String) {
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(problem);
        }
    }

    fn node(&mut self, v: &Value, at: &str, depth: usize) {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_NODES {
            if self.nodes == MAX_NODES.saturating_add(1) {
                self.say(format!("{at}: the schema has more than {MAX_NODES} parts"));
            }
            return;
        }
        if depth > MAX_DEPTH {
            self.say(format!("{at}: the schema nests deeper than {MAX_DEPTH} levels"));
            return;
        }
        let Some(o) = v.as_object() else {
            self.say(format!("{at}: a schema is an object"));
            return;
        };
        let Some(kind) = o.get("type").and_then(Value::as_str) else {
            self.say(format!("{at}: `type` is one type name"));
            return;
        };
        if !TYPES.contains(&kind) {
            self.say(format!("{at}: `{kind}` isn't a type of the subset"));
        }
        for (key, value) in o {
            self.keyword(kind, key, value, at, depth);
        }
        if kind == "object" {
            self.object(o, at);
        }
    }

    fn keyword(&mut self, kind: &str, key: &str, value: &Value, at: &str, depth: usize) {
        let next = depth.saturating_add(1);
        match (key, kind) {
            ("type", _) => {}
            ("properties", "object") => match value.as_object() {
                Some(props) => {
                    for (name, schema) in props {
                        self.node(schema, &format!("{at}/properties/{name}"), next);
                    }
                }
                None => self.say(format!("{at}: `properties` is an object of schemas")),
            },
            ("required", "object") => {
                if !value.as_array().is_some_and(|a| a.iter().all(Value::is_string)) {
                    self.say(format!("{at}: `required` is a list of property names"));
                }
            }
            ("additionalProperties", "object") => {
                if value.as_bool() != Some(false) {
                    self.say(format!("{at}: `additionalProperties` is only ever false"));
                }
            }
            ("enum", "string") => {
                if !value.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string)) {
                    self.say(format!("{at}: `enum` is a list of strings"));
                }
            }
            ("minimum" | "maximum", "number" | "integer") => {
                if !value.is_number() {
                    self.say(format!("{at}: `{key}` is a number"));
                }
            }
            ("minItems" | "maxItems", "array") => {
                if value.as_u64().is_none() {
                    self.say(format!("{at}: `{key}` is a whole number"));
                }
            }
            ("items", "array") => self.node(value, &format!("{at}/items"), next),
            _ => self.say(format!("{at}: `{key}` isn't in the portable subset for a `{kind}`")),
        }
    }

    /// An object names its properties, takes no others, and needs every one of them (as strict
    /// structured output does).
    fn object(&mut self, o: &serde_json::Map<String, Value>, at: &str) {
        if o.get("additionalProperties").and_then(Value::as_bool) != Some(false) {
            self.say(format!("{at}: an object says `\"additionalProperties\": false`"));
        }
        let names: Vec<&str> = o.get("properties").and_then(Value::as_object).map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default();
        let required: Vec<&str> =
            o.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        for name in &names {
            if !required.contains(name) {
                self.say(format!("{at}: property `{name}` is to be required"));
            }
        }
        for name in &required {
            if !names.contains(name) {
                self.say(format!("{at}: `{name}` is required but isn't a property"));
            }
        }
    }
}
