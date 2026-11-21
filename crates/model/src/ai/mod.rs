// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The data of MailCraft's AI features: plain types and pure functions, no I/O (L1, and in the
//! browser too).
//!
//! - **Retrieval** ([`retrieval`]): a request for the messages that answer a question, the
//!   scored hits that come back, each with the passage ([`Chunk`]) it was found in, and
//!   reciprocal rank fusion ([`fuse`]) to put several rankings together.
//! - **Passages** ([`chunk`]): a message's visible text (its subject and its body, never the
//!   words its sender hid) cut into passages at paragraph breaks, each known by its message and
//!   its place in that text.
//! - **Citations** ([`citation`]): what an answer names as its sources, checked against the
//!   messages that were given to the model ([`validate_citations`]): an id that wasn't given is
//!   refused, whatever the model says.
//! - **Find by Description** ([`narrow`]): the owner's words, and nothing of the mail, turned
//!   into a search the owner accepts; the proposal checked against the query language.
//! - **Answers** ([`prompt`]): the prompt of an answer from passages of mail, fitted to the
//!   model's context window, with the mail marked off from the instructions; made again by the
//!   same function for a backup model ([`AnswerInput`], [`ModelRequest::rebuilt_for`]).
//! - **Answers as JSON** ([`answer`], [`extract`]): `{"answer", "citations"}`, read and checked
//!   at the end ([`read_answer_json`]) and its text read as it streams ([`AnswerExtractor`]).
//! - **Structured answers** ([`schema`]): every JSON schema sent to a model, held to the subset
//!   of JSON Schema that every provider takes.
//! - **Runtimes** ([`runtime`]): the runtimes an answer may come from, the owner's chain of them
//!   and the fallback, where each sends what it is given ([`where_of`], never what a runtime says
//!   of itself), and per-account consent ([`may_send`]); turning a runtime on grants nothing.
//! - **Model calls** ([`call`]): what the engine asks a host to send to a model
//!   ([`ModelRequest`]), what the host hands back ([`ModelEvent`]; a failure with its
//!   [`ErrorKind`]), and the requests under way ([`Requests`]). The engine never talks to a
//!   model: a host with sockets does, and hands the result back (`ai.request`, `ai.apply`).
//!
//! **Message text is data, never instructions**, and so is what a model says: everything that
//! comes back from a model is checked and capped here ([`clean_text`]) before anything keeps it,
//! and nothing in it is run, fetched or sent.

pub mod answer;
pub mod call;
pub mod chunk;
pub mod citation;
pub mod extract;
pub mod narrow;
pub mod prompt;
pub mod retrieval;
pub mod runtime;
pub mod schema;

pub use answer::{
    ANSWER_SCHEMA, MAX_ANSWER_CITATIONS, MAX_ANSWER_JSON_BYTES, NOT_FOUND, StructuredAnswer, answer_schema, is_not_found, read_answer_json,
};
pub use call::{
    AiKind, AnsweredBy, Applied, ChatMessage, ErrorKind, MAX_ANSWERED_BY_BYTES, MAX_EVENTS, MAX_KEPT, MAX_LIVE, MAX_MESSAGES, MAX_PROMPT_BYTES,
    MAX_PROVIDER_TEXT_BYTES, MAX_RESPONSE_BYTES, MAX_SOURCES, ModelEvent, ModelRequest, RequestState, Requests, Role, SOURCES_UNREADABLE, Tracked,
    clean_provider_text, clean_text,
};
pub use chunk::{Chunk, MAX_CHUNK_CHARS, MAX_CHUNKS, MAX_TEXT_BYTES, chunks, searchable_text};
pub use citation::{Citation, MAX_CITATIONS, validate_citations};
pub use extract::{AnswerExtractor, extract_answer};
pub use narrow::{MAX_NARROW_CHARS, MAX_PROPOSAL_CHARS, NARROW_SCHEMA, check_query, narrow_messages, narrow_retry_messages, proposed_query};
pub use prompt::{
    AnswerInput, AnswerPrompt, Budget, DEFAULT_ANSWER_TOKENS, DEFAULT_CONTEXT_WINDOW, MAX_MARKER_BYTES, MAX_PASSAGES, MAX_QUESTION_CHARS, Passage,
    answer_prompt, answer_tokens_for, cited_numbers, estimate_tokens, marker,
};
pub use retrieval::{DEFAULT_LIMIT, MAX_LIMIT, MAX_QUERY_CHARS, RRF_K, RetrievalRequest, RetrievalResult, ScoredHit, fuse};
pub use runtime::{
    Carries, Chain, ChainEntry, CompatKind, CompatProvider, Consents, Fallback, MAX_CHAIN, ManagedUar, ModelName, ProviderId, RuntimeId, RuntimeKind,
    RuntimePath, UarEndpoint, UarInstance, Where, may_send, where_of,
};
pub use schema::{PRODUCT_SCHEMAS, portable_problems};

use crate::Id;

/// Why something of the AI features was refused. Its text is English, for whoever wrote what
/// was refused (a host, an agent, a script).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    /// A citation names a message that wasn't among those given to the model.
    #[error("message {0} was not among the messages given")]
    UnknownCitation(Id),
    /// More of something than is taken.
    #[error("too many {what}: at most {max}")]
    TooMany { what: &'static str, max: usize },
    /// A request or a result that isn't what it should be.
    #[error("{0}")]
    Invalid(String),
    /// No model request under way, or kept, has this id.
    #[error("no model request {0}")]
    UnknownRequest(u64),
    /// The model request is over (done or failed): nothing more is taken for it.
    #[error("model request {0} is finished")]
    Finished(u64),
}

#[cfg(test)]
mod answer_tests;
#[cfg(test)]
mod call_tests;
#[cfg(test)]
mod chunk_tests;
#[cfg(test)]
mod citation_tests;
#[cfg(test)]
mod extract_tests;
#[cfg(test)]
mod narrow_tests;
#[cfg(test)]
mod prompt_tests;
#[cfg(test)]
mod retrieval_tests;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod schema_tests;
