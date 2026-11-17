// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a provider says it serves (`GET /v1/models`), read as hostile input, and what that list
//! says about where a server set up as on this computer sends what it is given ([`route_of`]).
//!
//! **The list.** At most [`MAX_MODELS_BYTES`] are read and at most [`MAX_MODELS`] models kept; a
//! name is kept only when it is text of at most [`MAX_MODEL_ID`] characters with no control
//! characters, and a context window is clamped to 256 to 1,048,576 tokens. Anything else in an
//! entry is ignored, and an entry that isn't an object with an `id` is skipped. Nothing of it is
//! instructions; it is shown to the owner only as data.
//!
//! **The route check (two levels).** A gateway on this computer that forwards to a service on
//! the internet names its models after the routes it forwards them on (`anthropic/…`,
//! `alibaba-token/…`), while a server that runs models here names them after their publishers
//! (`llama3.2:latest`, `hf.co/…`, `lmstudio-community/…`, LM Studio's `openai/gpt-oss-20b`). So:
//!
//! 1. **Hosted route ([`Route::Hosted`]):** an id whose first part is one of [`HOSTED_ROUTES`] or
//!    ends in `-token`, or an `owned_by` naming one of those, is a hosted service's route as
//!    gateways write them, which no local server uses for its own models. A server set up as on
//!    this computer that lists one is sent nothing until the owner sets it up as a gateway.
//! 2. **Ask once ([`Route::Ask`]):** any other publisher prefix (`hf.co/`, `openai/`…), models
//!    owned by several publishers, models owned by `openai` (what LiteLLM and other proxies say;
//!    Ollama says `library`, LM Studio `organization_owner`, llama.cpp `llamacpp`, vLLM `vllm`),
//!    or an id of a hosted model family without a prefix ([`HOSTED_FAMILIES`]: `gpt-4o`,
//!    `claude-…`, the default list of a LiteLLM proxy). The owner is asked once whether the server
//!    sends mail to the internet, and the answer is kept for that address and that model list
//!    ([`fingerprint`]).
//!
//! Otherwise ([`Route::Local`]) the server is taken for what the owner set it up as. The lists are
//! a heuristic: a gateway that hides its routes isn't caught, which is why the owner's own
//! choice of kind stays the rule and this check is a second guard.

use std::fmt;

use serde_json::Value;

use crate::config::clamp_context_window;
use crate::error::{AiError, tidy};

/// Most bytes of a model list read.
pub const MAX_MODELS_BYTES: usize = 1024 * 1024;
/// Most models kept of a list.
pub const MAX_MODELS: usize = 1_000;
/// Longest model id kept, and longest `owned_by`, in characters.
pub const MAX_MODEL_ID: usize = 200;
const MAX_OWNER: usize = 100;
/// Most publishers named in a question.
const MAX_PUBLISHERS: usize = 8;

/// The first parts of model ids (and the `owned_by` names) that are routes to hosted services as
/// gateways write them, and that no server running models on this computer uses for its own.
/// `openai/`, `mistral/` and `deepseek/` are not here: LM Studio names local models that way.
pub const HOSTED_ROUTES: [&str; 8] = ["anthropic", "azure", "bedrock", "vertex_ai", "gemini", "groq", "openrouter", "dashscope"];

/// The starts of ids of model families that are served only by hosted services (a server on this
/// computer that lists one under that name is, as a rule, a proxy): the owner is asked
/// ([`Route::Ask`]). `o1`, `o3` and `o4` count only alone or followed by `-` ([`hosted_family`]).
pub const HOSTED_FAMILIES: [&str; 13] = [
    "gpt-",
    "claude-",
    "gemini-",
    "qwen-max",
    "qwen-plus",
    "qwen-turbo",
    "qwen3-max",
    "qwen3.8-max",
    "qwen4-max",
    "deepseek-chat",
    "deepseek-reasoner",
    "mistral-large",
    "command-r",
];

/// The `owned_by` a proxy in front of hosted services writes (no server running models here does).
const PROXY_OWNER: &str = "openai";

/// One model a provider says it serves.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ModelInfo {
    /// Its name, as requests name it.
    pub id: String,
    /// Who the provider says it belongs to, tidied; none when it said nothing.
    pub owned_by: Option<String>,
    /// How many tokens it takes, when the list says (`context_length`, `context_window` or
    /// `max_model_len`), clamped to 256 to 1,048,576.
    pub context_window: Option<u32>,
}

impl fmt::Debug for ModelInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelInfo").field("id_len", &self.id.len()).field("context_window", &self.context_window).finish()
    }
}

/// What a provider says it serves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelList {
    pub models: Vec<ModelInfo>,
    /// The list had more than [`MAX_MODELS`] models: the rest weren't read.
    pub truncated: bool,
}

impl ModelList {
    /// The model named `id`, when the list has it.
    pub fn find(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == id.trim())
    }
}

/// Whether `c` is a format character that changes how a name looks, or hides part of it, without
/// being seen: zero-width spaces and joiners, direction marks, embeddings and isolates, the word
/// joiner and invisible operators, the Arabic letter mark and the byte order mark.
pub(crate) fn is_hidden_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}')
}

/// A short name from a list (an id, an owner): text alone, without control characters, with its
/// hidden format characters ([`is_hidden_format`]) taken out (so `anthro\u{200B}pic/…` is read
/// as the route it shows), trimmed, at most `max` characters; none when empty or too long to be
/// one.
fn name(value: Option<&Value>, max: usize) -> Option<String> {
    let raw = value?.as_str()?;
    if raw.chars().any(char::is_control) {
        return None;
    }
    let shown: String = raw.chars().filter(|c| !is_hidden_format(*c)).collect();
    let text = shown.trim();
    if text.is_empty() || text.chars().count() > max {
        return None;
    }
    Some(text.to_string())
}

/// A context window from an entry, clamped to its bounds as any number from outside.
fn context_of(entry: &Value) -> Option<u32> {
    ["context_length", "context_window", "max_model_len"].iter().find_map(|field| entry.get(*field).and_then(Value::as_u64)).map(clamp_context_window)
}

/// A `GET /v1/models` answer: `{"data": [{"id", "owned_by"?, …}]}` (or `{"models": […]}`), as far
/// as it can be read. An error in the answer is the provider's; an answer that has no list isn't one.
pub fn parse_models(body: &[u8]) -> Result<ModelList, AiError> {
    if body.len() > MAX_MODELS_BYTES {
        return Err(AiError::TooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| AiError::Malformed("it isn't the JSON a list of models is".into()))?;
    if let Some(message) = crate::wire::error_message(&value) {
        return Err(AiError::Provider(message));
    }
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::Malformed("it has no list of models in it".into()))?;
    let mut list = ModelList::default();
    for entry in entries {
        if list.models.len() >= MAX_MODELS {
            list.truncated = true;
            break;
        }
        let Some(id) = name(entry.get("id"), MAX_MODEL_ID) else { continue };
        if list.models.iter().any(|m| m.id == id) {
            continue;
        }
        let owned_by = name(entry.get("owned_by"), MAX_OWNER).map(|owner| tidy(&owner, None));
        list.models.push(ModelInfo { id, owned_by, context_window: context_of(entry) });
    }
    Ok(list)
}

/// What a model list says about a server set up as on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Nothing says otherwise: a server running models here.
    Local,
    /// Several publishers, or a publisher's namespace: the owner is asked once. `publishers`
    /// names (at most eight of) them, as the server wrote them: data to show, never instructions.
    Ask { publishers: Vec<String> },
    /// A hosted service's route: refused as on this computer. `listed` is the first id (or owner)
    /// that says so, as the server wrote it.
    Hosted { listed: String },
}

/// Whether `prefix` (lower case) names a hosted service's route.
fn hosted(prefix: &str) -> bool {
    HOSTED_ROUTES.contains(&prefix) || (prefix.len() > "-token".len() && prefix.ends_with("-token"))
}

/// Whether `id` (without a publisher prefix) names a model of a hosted family ([`HOSTED_FAMILIES`],
/// or `o1`, `o3`, `o4` alone or followed by `-`).
fn hosted_family(id: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    let o_series = ["o1", "o3", "o4"].iter().any(|o| id.strip_prefix(o).is_some_and(|rest| rest.is_empty() || rest.starts_with('-')));
    o_series || HOSTED_FAMILIES.iter().any(|family| id.starts_with(family))
}

/// The publisher (or route) part of a model id, lower case: what comes before its first `/`.
fn prefix_of(id: &str) -> Option<String> {
    let (prefix, rest) = id.split_once('/')?;
    let prefix = prefix.trim().to_ascii_lowercase();
    (!prefix.is_empty() && !rest.trim().is_empty()).then_some(prefix)
}

/// The route check of a server set up as on this computer (see the module's notes).
pub fn route_of(list: &ModelList) -> Route {
    for model in &list.models {
        if prefix_of(&model.id).is_some_and(|prefix| hosted(&prefix)) {
            return Route::Hosted { listed: model.id.clone() };
        }
        if let Some(owner) = &model.owned_by
            && hosted(&owner.trim().to_ascii_lowercase())
        {
            return Route::Hosted { listed: owner.clone() };
        }
    }
    let mut publishers: Vec<String> = Vec::new();
    let mut add = |publisher: String| {
        if publishers.len() < MAX_PUBLISHERS && !publishers.iter().any(|kept| kept.eq_ignore_ascii_case(&publisher)) {
            publishers.push(publisher);
        }
    };
    let mut prefixed = false;
    let mut family = false;
    for model in &list.models {
        if let Some(prefix) = prefix_of(&model.id) {
            prefixed = true;
            add(prefix);
        } else if hosted_family(&model.id) {
            family = true;
            add(model.id.clone());
        }
    }
    let mut owners: Vec<String> = list.models.iter().filter_map(|m| m.owned_by.as_ref()).map(|owner| owner.trim().to_ascii_lowercase()).collect();
    owners.sort();
    owners.dedup();
    let several_owners = owners.len() > 1;
    let proxy_owner = owners.iter().any(|owner| owner == PROXY_OWNER);
    if several_owners || proxy_owner {
        for owner in owners {
            add(owner);
        }
    }
    if prefixed || family || several_owners || proxy_owner { Route::Ask { publishers } } else { Route::Local }
}

/// A short, stable name for a model list (its ids, without regard to their order or case): the
/// owner's answer to the question of [`Route::Ask`] is kept for that address and this name, so a
/// list that changes is asked about again. Sixteen hexadecimal digits (FNV-1a, 64 bits): it tells
/// lists apart, it isn't a secret.
pub fn fingerprint(list: &ModelList) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut ids: Vec<String> = list.models.iter().map(|m| m.id.to_ascii_lowercase()).collect();
    ids.sort();
    ids.dedup();
    let mut hash = OFFSET;
    for id in &ids {
        for byte in id.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{hash:016x}")
}

/// Whether `text` is one [`fingerprint`] could have made.
pub fn is_fingerprint(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| b.is_ascii_hexdigit())
}
