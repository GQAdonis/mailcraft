// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The structured-output ladder: what each level asks, which errors step down, how a plain answer
//! is read, and that the level that worked is kept for the provider.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use serde_json::json;

use crate::error::AiError;
use crate::structured::{Ladders, Level, at_level, read_answer, steps_down};
use crate::wire::{ChatRequest, Message, ResponseFormat, Role, chat_body};

fn request(with_system: bool) -> ChatRequest {
    let mut messages = vec![Message::user("unread from Ana")];
    if with_system {
        messages.insert(0, Message::system("Propose a search."));
    }
    ChatRequest {
        messages,
        max_tokens: Some(64),
        response_format: Some(ResponseFormat::JsonSchema {
            name: "mail_search".into(),
            schema: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            strict: true,
        }),
        ..ChatRequest::default()
    }
}

#[test]
fn each_level_asks_its_own_way() {
    let top = at_level(&request(true), Level::JsonSchema);
    assert_eq!(top, request(true));

    let object = at_level(&request(true), Level::JsonObject);
    assert_eq!(object.response_format, Some(ResponseFormat::JsonObject));
    assert_eq!(object.messages.len(), 2);
    assert!(object.messages[0].content.starts_with("Propose a search.\n\n"));
    assert!(object.messages[0].content.contains("json object"), "Alibaba's json_object needs the word json");
    assert!(object.messages[0].content.contains(r#""required":["query"]"#));
    let written = String::from_utf8(chat_body("m", &object, false).unwrap()).unwrap();
    assert!(written.contains(r#""response_format":{"type":"json_object"}"#), "{written}");

    let plain = at_level(&request(true), Level::Plain);
    assert_eq!(plain.response_format, None);
    assert!(plain.messages[0].content.contains("json object and nothing else"));
    let written = String::from_utf8(chat_body("m", &plain, false).unwrap()).unwrap();
    assert!(!written.contains("response_format"), "{written}");
}

#[test]
fn without_a_system_message_one_is_put_first() {
    for level in [Level::JsonObject, Level::Plain] {
        let asked = at_level(&request(false), level);
        assert_eq!(asked.messages.len(), 2);
        assert_eq!(asked.messages[0].role, Role::System);
        assert!(asked.messages[0].content.contains("json"));
        assert_eq!(asked.messages[1].content, "unread from Ana");
    }
}

#[test]
fn only_a_400_to_a_response_format_steps_down() {
    let status = |status: u16| AiError::Status { status, message: "Json mode response is not supported when enable_thinking is true".into() };
    assert!(steps_down(&status(400), Level::JsonSchema));
    assert!(steps_down(&status(400), Level::JsonObject));
    assert!(!steps_down(&status(400), Level::Plain), "the end of the ladder");
    for other in [401, 403, 404, 429, 500, 502, 503] {
        assert!(!steps_down(&status(other), Level::JsonSchema), "{other}");
    }
    for error in
        [AiError::Unreachable(String::new()), AiError::TimedOut(String::new()), AiError::Malformed(String::new()), AiError::Provider("400".into())]
    {
        assert!(!steps_down(&error, Level::JsonSchema), "{error:?}");
    }
    assert_eq!(Level::JsonSchema.next(), Some(Level::JsonObject));
    assert_eq!(Level::JsonObject.next(), Some(Level::Plain));
    assert_eq!(Level::Plain.next(), None);
}

#[test]
fn a_plain_answer_is_read_where_its_object_is() {
    assert_eq!(read_answer(r#"{"query":"from:ana"}"#, Level::JsonSchema).unwrap(), json!({"query": "from:ana"}));
    assert_eq!(read_answer("```json\n{\"query\":\"a\"}\n```", Level::JsonObject).unwrap(), json!({"query": "a"}));
    let chatty = "Sure! Here is the search: {\"query\": \"from:ana is:unread\"} Hope it helps.";
    assert_eq!(read_answer(chatty, Level::Plain).unwrap(), json!({"query": "from:ana is:unread"}));
    assert!(read_answer(chatty, Level::JsonObject).is_err(), "only the plain level reads around words");
    for bad in ["no json here", "} backwards {", "[1, 2]", "{not: json}", "über {"] {
        assert!(read_answer(bad, Level::Plain).is_err(), "{bad}");
    }
}

#[test]
fn the_level_that_worked_is_kept_for_the_provider() {
    let ladders = Ladders::default();
    assert_eq!(ladders.start("a"), Level::JsonSchema);
    ladders.keep("a", Level::Plain);
    ladders.keep("b", Level::JsonObject);
    assert_eq!((ladders.start("a"), ladders.start("b"), ladders.start("c")), (Level::Plain, Level::JsonObject, Level::JsonSchema));
    ladders.keep("a", Level::JsonSchema);
    assert_eq!(ladders.start("a"), Level::JsonSchema);
    for n in 0..200 {
        ladders.keep(&format!("p{n}"), Level::Plain);
    }
    assert_eq!(ladders.start("b"), Level::JsonObject, "the cap never forgets what is kept");
}

proptest! {
    #[test]
    fn any_answer_is_json_or_an_error_never_a_panic(text in ".{0,300}") {
        for level in [Level::JsonSchema, Level::JsonObject, Level::Plain] {
            let _ = read_answer(&text, level);
        }
    }
}
