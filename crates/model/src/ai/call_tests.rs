// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::call::*;

fn user(text: &str) -> ChatMessage {
    ChatMessage { role: Role::User, content: text.into() }
}

fn request() -> ModelRequest {
    ModelRequest::new(AiKind::Answer, vec![user("What deposit did Harbor Hall ask for?")], vec![3, 4, 3], true, None, Some(0.2), None).unwrap()
}

#[test]
fn a_request_is_checked_and_brought_within_range() {
    let r = request();
    assert_eq!(r.sources, [3, 4], "sources once each");
    assert_eq!((r.id, r.max_tokens, r.temperature), (0, 1_024, 0.2));
    let r = ModelRequest::new(AiKind::Narrow, vec![user("x")], vec![], false, Some(u32::MAX), Some(f32::NAN), None).unwrap();
    assert_eq!((r.max_tokens, r.temperature), (8_192, 0.0));
    let r = ModelRequest::new(AiKind::Narrow, vec![user("x")], vec![], false, Some(0), Some(f32::INFINITY), None).unwrap();
    assert_eq!((r.max_tokens, r.temperature), (1, 0.0));
    assert!(ModelRequest::new(AiKind::Answer, vec![], vec![], false, None, None, None).is_err());
    assert!(ModelRequest::new(AiKind::Answer, vec![user("x"); MAX_MESSAGES + 1], vec![], false, None, None, None).is_err());
    assert!(ModelRequest::new(AiKind::Answer, vec![user(&"x".repeat(MAX_PROMPT_BYTES + 1))], vec![], false, None, None, None).is_err());
    assert!(ModelRequest::new(AiKind::Answer, vec![user("x")], (1..=MAX_SOURCES as u64 + 1).collect(), false, None, None, None).is_err());
    assert!(ModelRequest::new(AiKind::Answer, vec![user("x")], vec![0], false, None, None, None).is_err());
    assert!(ModelRequest::new(AiKind::Answer, vec![user("x")], vec![], false, None, None, Some("{".repeat(20_000))).is_err());
    // Control characters in what is sent are dropped.
    let r = ModelRequest::new(AiKind::Answer, vec![user("a\u{0}b\u{202e}c\nd")], vec![], false, None, None, None).unwrap();
    assert_eq!(r.messages[0].content, "abc\nd");
}

#[test]
fn a_streamed_answer_comes_in_pieces_then_ends() {
    let mut reqs = Requests::default();
    let r = reqs.start(request()).unwrap();
    assert_eq!(r.id, 1);
    assert_eq!(reqs.get(1).unwrap().state, RequestState::Pending);
    assert_eq!(reqs.apply(1, ModelEvent::Delta { text: "The deposit ".into() }), Ok(Applied::Taken(RequestState::Streaming)));
    assert_eq!(reqs.apply(1, ModelEvent::Delta { text: "is 500 euros.".into() }), Ok(Applied::Taken(RequestState::Streaming)));
    assert_eq!(reqs.get(1).unwrap().text, "The deposit is 500 euros.");
    assert_eq!(reqs.apply(1, ModelEvent::done(None)), Ok(Applied::Taken(RequestState::Done)));
    assert_eq!(reqs.get(1).unwrap().text, "The deposit is 500 euros.");
    // Over: nothing more is taken for it.
    assert_eq!(reqs.apply(1, ModelEvent::Delta { text: "more".into() }), Err(AiError::Finished(1)));
    assert_eq!(reqs.get(1).unwrap().text, "The deposit is 500 euros.");
}

#[test]
fn the_whole_answer_at_the_end_takes_the_place_of_the_pieces() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    reqs.apply(id, ModelEvent::Delta { text: "draft".into() }).unwrap();
    reqs.apply(id, ModelEvent::done(Some("final\u{0} answer".into()))).unwrap();
    assert_eq!(reqs.get(id).unwrap().text, "final answer");
}

#[test]
fn an_error_fails_the_request_in_words_capped() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    reqs.apply(id, ModelEvent::Error { kind: ErrorKind::Unreachable, message: "x".repeat(10_000), provider_text: None }).unwrap();
    let t = reqs.get(id).unwrap();
    assert_eq!(t.state, RequestState::Failed);
    assert!(t.error.len() <= 600);
    assert_eq!((t.error_kind, t.provider_text.as_str()), (Some(ErrorKind::Unreachable), ""));
}

#[test]
fn a_failure_keeps_its_kind_and_the_providers_words_as_bounded_data() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    let said = format!("\u{202e}\"maxLength\" at #/properties/query\r\ncannot be enforced\u{0}{}", "é".repeat(400));
    let event = ModelEvent::Error {
        kind: ErrorKind::Refused { status: 400 },
        message: "The provider refused the request.".into(),
        provider_text: Some(said),
    };
    assert_eq!(reqs.apply(id, event), Ok(Applied::Taken(RequestState::Failed)));
    let t = reqs.get(id).unwrap();
    assert_eq!(t.error_kind, Some(ErrorKind::Refused { status: 400 }));
    assert!(t.provider_text.starts_with("\"maxLength\" at #/properties/query  cannot be enforced"), "{}", t.provider_text);
    assert!(t.provider_text.len() <= MAX_PROVIDER_TEXT_BYTES);
    assert!(!t.provider_text.chars().any(|c| c.is_control() || c == '\u{202e}'));
    assert_eq!(t.error, "The provider refused the request.");
}

#[test]
fn too_many_events_fail_as_too_large() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    for _ in 0..=MAX_EVENTS {
        let _ = reqs.apply(id, ModelEvent::Delta { text: String::new() });
    }
    assert_eq!(reqs.get(id).unwrap().error_kind, Some(ErrorKind::TooLarge));
}

#[test]
fn clean_provider_text_is_one_line_capped_at_a_character_boundary() {
    assert_eq!(clean_provider_text("a\nb\tc\u{1b}d\u{2066}e"), "a b c de");
    let long = "é".repeat(MAX_PROVIDER_TEXT_BYTES);
    let kept = clean_provider_text(&long);
    assert_eq!(kept.len(), MAX_PROVIDER_TEXT_BYTES);
    assert!(kept.chars().all(|c| c == 'é'));
    let odd = format!("x{}", "é".repeat(MAX_PROVIDER_TEXT_BYTES));
    assert!(clean_provider_text(&odd).len() < MAX_PROVIDER_TEXT_BYTES);
    assert_eq!(clean_provider_text(""), "");
}

#[test]
fn error_kinds_are_read_and_written_as_snake_case_json() {
    let e: ModelEvent =
        serde_json::from_str(r#"{"type": "error", "kind": {"refused": {"status": 400}}, "message": "m", "providerText": "p"}"#).unwrap();
    assert_eq!(e, ModelEvent::Error { kind: ErrorKind::Refused { status: 400 }, message: "m".into(), provider_text: Some("p".into()) });
    let e: ModelEvent = serde_json::from_str(r#"{"type": "error", "kind": "credential_store", "message": "m"}"#).unwrap();
    assert_eq!(e, ModelEvent::Error { kind: ErrorKind::CredentialStore, message: "m".into(), provider_text: None });
    let e: ModelEvent = serde_json::from_str(r#"{"type": "error", "kind": {"timed_out": {"before_first_byte": true}}, "message": "m"}"#).unwrap();
    assert_eq!(e, ModelEvent::Error { kind: ErrorKind::TimedOut { before_first_byte: true }, message: "m".into(), provider_text: None });
    // Every kind written is read back the same, and is called by its name.
    let all = [
        ErrorKind::NotConfigured,
        ErrorKind::NoConsent,
        ErrorKind::Unreachable,
        ErrorKind::Refused { status: 503 },
        ErrorKind::TimedOut { before_first_byte: false },
        ErrorKind::Certificate,
        ErrorKind::Settings,
        ErrorKind::CredentialStore,
        ErrorKind::Malformed,
        ErrorKind::TooLarge,
        ErrorKind::Cancelled,
    ];
    for kind in all {
        let v = serde_json::to_value(kind).unwrap();
        let name = v.as_str().map(str::to_string).or_else(|| v.as_object().and_then(|o| o.keys().next().cloned())).unwrap();
        assert_eq!(name, kind.name());
        assert_eq!(serde_json::from_value::<ErrorKind>(v).unwrap(), kind);
    }
    // No kind, an unknown kind, or details that aren't what they should be: refused.
    for bad in [
        r#"{"type": "error", "message": "m"}"#,
        r#"{"type": "error", "kind": "on_fire", "message": "m"}"#,
        r#"{"type": "error", "kind": {"refused": {"status": 70000}}, "message": "m"}"#,
        r#"{"type": "error", "kind": {"timed_out": {"before_first_byte": "yes"}}, "message": "m"}"#,
        r#"{"type": "error", "kind": "unreachable", "message": "m", "providerText": 5}"#,
    ] {
        assert!(serde_json::from_str::<ModelEvent>(bad).is_err(), "{bad}");
    }
}

#[test]
fn which_kinds_fall_back_to_another_provider() {
    for kind in [
        ErrorKind::NotConfigured,
        ErrorKind::NoConsent,
        ErrorKind::Unreachable,
        ErrorKind::Refused { status: 400 },
        ErrorKind::TimedOut { before_first_byte: true },
        ErrorKind::Certificate,
        ErrorKind::Settings,
        ErrorKind::CredentialStore,
    ] {
        assert!(kind.falls_back(), "{kind:?}");
    }
    for kind in [ErrorKind::TimedOut { before_first_byte: false }, ErrorKind::Malformed, ErrorKind::TooLarge, ErrorKind::Cancelled] {
        assert!(!kind.falls_back(), "{kind:?}");
    }
}

#[test]
fn a_cancelled_request_lets_go_of_what_comes_later() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    reqs.apply(id, ModelEvent::Delta { text: "partial".into() }).unwrap();
    assert_eq!(reqs.cancel(id), Ok(true));
    assert_eq!(reqs.apply(id, ModelEvent::Delta { text: " more".into() }), Ok(Applied::Ignored));
    assert_eq!(reqs.apply(id, ModelEvent::done(Some("x".into()))), Ok(Applied::Ignored));
    let t = reqs.get(id).unwrap();
    assert_eq!((t.state, t.text.as_str()), (RequestState::Cancelled, "partial"));
    // Calling it off again does nothing.
    assert_eq!(reqs.cancel(id), Ok(false));
}

#[test]
fn unknown_and_forgotten_requests_are_refused() {
    let mut reqs = Requests::default();
    assert_eq!(reqs.apply(9, ModelEvent::done(None)), Err(AiError::UnknownRequest(9)));
    assert_eq!(reqs.cancel(0), Err(AiError::UnknownRequest(0)));
    // The oldest finished goes once MAX_KEPT are kept; its id is unknown after.
    for _ in 0..MAX_KEPT {
        let id = reqs.start(request()).unwrap().id;
        reqs.apply(id, ModelEvent::done(None)).unwrap();
    }
    reqs.start(request()).unwrap();
    assert!(reqs.all().len() <= MAX_KEPT);
    assert_eq!(reqs.apply(1, ModelEvent::done(None)), Err(AiError::UnknownRequest(1)));
}

#[test]
fn requests_under_way_are_capped() {
    let mut reqs = Requests::default();
    for _ in 0..MAX_LIVE {
        reqs.start(request()).unwrap();
    }
    assert!(matches!(reqs.start(request()), Err(AiError::TooMany { .. })));
    assert_eq!(reqs.live(), MAX_LIVE);
}

#[test]
fn the_answer_is_capped_and_marked_cut() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    for _ in 0..3 {
        reqs.apply(id, ModelEvent::Delta { text: "é".repeat(MAX_RESPONSE_BYTES / 2) }).unwrap();
    }
    let t = reqs.get(id).unwrap();
    assert!(t.text.len() <= MAX_RESPONSE_BYTES && t.truncated);
}

#[test]
fn too_many_events_fail_the_request() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    for _ in 0..MAX_EVENTS {
        reqs.apply(id, ModelEvent::Delta { text: String::new() }).unwrap();
    }
    assert_eq!(reqs.apply(id, ModelEvent::Delta { text: "x".into() }), Ok(Applied::Taken(RequestState::Failed)));
    assert_eq!(reqs.apply(id, ModelEvent::Delta { text: "x".into() }), Err(AiError::Finished(id)));
}

#[test]
fn clean_text_drops_controls_and_direction_marks_and_caps() {
    assert_eq!(clean_text("a\u{0}\u{1b}[31mb\u{2066}c\td\ne", 100), ("a[31mbc\td\ne".to_string(), false));
    assert_eq!(clean_text("ééé", 5), ("éé".to_string(), true));
    assert_eq!(clean_text("", 0), (String::new(), false));
}

#[test]
fn events_are_read_from_a_hosts_json() {
    let e: ModelEvent = serde_json::from_str(r#"{"type": "delta", "text": "hi"}"#).unwrap();
    assert_eq!(e, ModelEvent::Delta { text: "hi".into() });
    let e: ModelEvent = serde_json::from_str(r#"{"type": "done"}"#).unwrap();
    assert_eq!(e, ModelEvent::done(None));
    assert!(serde_json::from_str::<ModelEvent>(r#"{"type": "run", "command": "mail.send"}"#).is_err());
}

proptest::proptest! {
    #[test]
    fn any_events_keep_the_bounds(events in proptest::collection::vec((0u8..4, "\\PC{0,300}"), 0..60)) {
        let mut reqs = Requests::default();
        let id = reqs.start(request()).unwrap().id;
        for (kind, text) in events {
            let _ = match kind {
                0 | 1 => reqs.apply(id, ModelEvent::Delta { text }),
                2 => reqs.apply(id, ModelEvent::done(Some(text))),
                _ => reqs.apply(id.wrapping_add(1), ModelEvent::Error { kind: ErrorKind::Malformed, message: text.clone(), provider_text: Some(text) }),
            };
            let t = reqs.get(id).unwrap();
            proptest::prop_assert!(t.text.len() <= MAX_RESPONSE_BYTES);
            proptest::prop_assert!(!t.text.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }
    }
}

#[test]
fn debug_shows_lengths_and_ids_never_text() {
    let r =
        ModelRequest::new(AiKind::Answer, vec![user("the deposit is 500 euros")], vec![7], true, None, None, Some("{\"secret\":1}".into())).unwrap();
    let mut reqs = Requests::default();
    let id = reqs.start(r.clone()).unwrap().id;
    reqs.apply(id, ModelEvent::Delta { text: "PARTIAL-ANSWER".into() }).unwrap();
    let failed =
        ModelEvent::Error { kind: ErrorKind::Refused { status: 400 }, message: "SERVER-WORDS".into(), provider_text: Some("PROVIDER-WORDS".into()) };
    let shown = format!("{r:?} {failed:?} {:?} {reqs:?}", r.messages[0]);
    for secret in ["500 euros", "secret", "PARTIAL-ANSWER", "SERVER-WORDS", "PROVIDER-WORDS"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(shown.contains("bytes") && shown.contains("sources: [7]"), "{shown}");
}

fn passage(message: u64, text: &str) -> super::prompt::Passage {
    super::prompt::Passage {
        message,
        subject: format!("Subject {message}"),
        from: "Harbor Hall Events <events@harborhall.example>".into(),
        date: "2026-10-14".into(),
        text: text.into(),
    }
}

/// An answer request asked for as JSON, carrying its passages, as `ai.ask` makes it.
fn answer_request(sources: Vec<u64>, window: usize) -> ModelRequest {
    let input = super::prompt::AnswerInput {
        question: "What deposit did Harbor Hall ask for?".into(),
        passages: sources.iter().map(|id| passage(*id, &"We ask for a deposit of 500 euros. ".repeat(30))).collect(),
        marker: super::prompt::marker(9),
    };
    let prompt = input.prompt(window).unwrap();
    let schema = Some(super::answer::answer_schema(prompt.sources.len()));
    ModelRequest::new(AiKind::Answer, prompt.messages, prompt.sources, true, Some(512), Some(0.2), schema).unwrap().with_answer(input).unwrap()
}

#[test]
fn the_end_says_who_answered_and_an_older_end_without_it_is_taken() {
    let mut reqs = Requests::default();
    let id = reqs.start(request()).unwrap().id;
    let e: ModelEvent = serde_json::from_str(
        r#"{"type": "done", "text": "500 euros [1]", "answeredBy": {"provider": "apfel\r\nX", "model": "apple-foundationmodel\u202e"}}"#,
    )
    .unwrap();
    assert_eq!(reqs.apply(id, e), Ok(Applied::Taken(RequestState::Done)));
    assert_eq!(reqs.get(id).unwrap().answered_by, Some(AnsweredBy { provider: "apfel  X".into(), model: "apple-foundationmodel".into() }));
    // Written back the way it was read.
    let by = Some(AnsweredBy { provider: "p".into(), model: "m".into() });
    let back = serde_json::to_value(ModelEvent::Done { text: None, answered_by: by, sources: None }).unwrap();
    assert_eq!(back, serde_json::json!({"type": "done", "text": null, "answeredBy": {"provider": "p", "model": "m"}}));
    // An older host's end, without it.
    let id = reqs.start(request()).unwrap().id;
    let e: ModelEvent = serde_json::from_str(r#"{"type": "done", "text": "x"}"#).unwrap();
    reqs.apply(id, e).unwrap();
    assert_eq!(reqs.get(id).unwrap().answered_by, None);
    // Nothing of who: none kept; and capped.
    assert_eq!(AnsweredBy { provider: " ".into(), model: "\u{0}".into() }.cleaned(), None);
    let long = AnsweredBy { provider: "p".repeat(10_000), model: "é".repeat(10_000) }.cleaned().unwrap();
    assert!(long.provider.len() <= MAX_ANSWERED_BY_BYTES && long.model.len() <= MAX_ANSWERED_BY_BYTES);
}

#[test]
fn an_answer_as_json_keeps_its_text_and_its_citations_apart() {
    let mut reqs = Requests::default();
    let id = reqs.start(answer_request(vec![3, 4, 5], 100_000)).unwrap().id;
    reqs.apply(id, ModelEvent::Delta { text: "The deposit is 500 euros".into() }).unwrap();
    let json = r#"{"answer": "The deposit is 500 euros [1].\u0000", "citations": [1, 1, 9, "2", 0, 3, 2], "extra": true}"#;
    assert_eq!(reqs.apply(id, ModelEvent::done(Some(json.into()))), Ok(Applied::Taken(RequestState::Done)));
    let t = reqs.get(id).unwrap();
    assert_eq!(t.text, "The deposit is 500 euros [1].");
    assert_eq!(t.cited, Some(vec![1, 3, 2]));
    assert_eq!(t.cited_dropped, 3);
    // A plain answer to a request without a schema is read as before.
    let id = reqs.start(request()).unwrap().id;
    reqs.apply(id, ModelEvent::done(Some(r#"{"answer": "x", "citations": [1]}"#.into()))).unwrap();
    assert_eq!(reqs.get(id).unwrap().cited, None);
    // A plain answer to a request with one (the provider's last step down): no citations read here.
    let id = reqs.start(answer_request(vec![3], 100_000)).unwrap().id;
    reqs.apply(id, ModelEvent::done(Some("The deposit is 500 euros (1).".into()))).unwrap();
    let t = reqs.get(id).unwrap();
    assert_eq!((t.state, t.cited.as_ref(), t.text.as_str()), (RequestState::Done, None, "The deposit is 500 euros (1)."));
}

#[test]
fn an_answer_whose_json_cant_be_read_fails_as_malformed_keeping_what_was_shown() {
    let mut reqs = Requests::default();
    let id = reqs.start(answer_request(vec![3, 4], 100_000)).unwrap().id;
    reqs.apply(id, ModelEvent::Delta { text: "The deposit is 500".into() }).unwrap();
    assert_eq!(reqs.apply(id, ModelEvent::done(Some(r#"{"answer": "The deposit is 500 eu"#.into()))), Ok(Applied::Taken(RequestState::Failed)));
    let t = reqs.get(id).unwrap();
    assert_eq!((t.error_kind, t.text.as_str(), t.error.as_str()), (Some(ErrorKind::Malformed), "The deposit is 500", SOURCES_UNREADABLE));
    assert!(!ErrorKind::Malformed.falls_back());
    // Nothing shown: as much of the answer as can be read.
    let id = reqs.start(answer_request(vec![3], 100_000)).unwrap().id;
    reqs.apply(id, ModelEvent::done(Some(r#"{"citations": [1], "answer": 5}"#.into()))).unwrap();
    assert_eq!((reqs.get(id).unwrap().state, reqs.get(id).unwrap().text.as_str()), (RequestState::Failed, ""));
    let id = reqs.start(answer_request(vec![3], 100_000)).unwrap().id;
    reqs.apply(id, ModelEvent::done(Some(r#"{"answer": "Half an ans"#.into()))).unwrap();
    let t = reqs.get(id).unwrap();
    assert_eq!((t.state, t.text.as_str()), (RequestState::Failed, "Half an ans"));
}

#[test]
fn a_backup_prompt_is_made_again_by_the_same_builder_and_the_end_names_its_messages() {
    let r = answer_request(vec![3, 4, 5, 6], 100_000);
    assert_eq!(r.sources, [3, 4, 5, 6]);
    let mut reqs = Requests::default();
    let started = reqs.start(r).unwrap();
    // A smaller window: fewer messages, fewer tokens, the schema for them, the same id.
    let again = started.rebuilt_for(1_024).unwrap();
    assert_eq!(again.id, started.id);
    assert!(!again.sources.is_empty() && again.sources.len() < 4, "{:?}", again.sources);
    assert!(again.sources.iter().all(|s| started.sources.contains(s)));
    assert!(again.max_tokens <= started.max_tokens && again.max_tokens as usize == super::prompt::answer_tokens_for(1_024));
    assert_eq!(again.schema, Some(super::answer::answer_schema(again.sources.len())));
    assert_eq!(again.answer, started.answer);
    let total: usize = again.messages.iter().map(|m| super::prompt::estimate_tokens(&m.content)).sum();
    assert!(total + again.max_tokens as usize <= 1_024 + 16, "{total}");
    // Made again for the same window, it is the same prompt.
    assert_eq!(started.rebuilt_for(100_000).unwrap().messages, started.messages);
    // The end names the rebuilt prompt's messages: citations are read against them.
    let event = ModelEvent::Done {
        text: Some(r#"{"answer": "500 euros [1]", "citations": [1]}"#.into()),
        answered_by: Some(AnsweredBy { provider: "apfel".into(), model: "apple-foundationmodel".into() }),
        sources: Some(vec![started.sources[1]]),
    };
    reqs.apply(started.id, event).unwrap();
    let t = reqs.get(started.id).unwrap();
    assert_eq!((t.sources(), t.cited.as_deref()), (&[started.sources[1]][..], Some(&[1usize][..])));
    // A request that carries no passages can't be made again.
    assert!(request().rebuilt_for(100_000).is_err());
    assert!(started.rebuilt_for(10).is_err());
}

#[test]
fn the_messages_an_end_names_are_the_requests_or_it_is_refused_unchanged() {
    let mut reqs = Requests::default();
    let id = reqs.start(answer_request(vec![3, 4], 100_000)).unwrap().id;
    for bad in [vec![], vec![9], vec![3, 3], vec![0], (1..=MAX_SOURCES as u64 + 1).collect()] {
        let event = ModelEvent::Done { text: Some("x".into()), answered_by: None, sources: Some(bad.clone()) };
        assert!(matches!(reqs.apply(id, event), Err(AiError::Invalid(_))), "{bad:?}");
    }
    let t = reqs.get(id).unwrap();
    assert_eq!((t.state, t.events, t.sources_used.as_ref()), (RequestState::Pending, 0, None));
}

#[test]
fn passages_are_checked_against_the_request() {
    let base = || ModelRequest::new(AiKind::Answer, vec![user("q")], vec![3, 4], true, None, None, None).unwrap();
    let input = |passages: Vec<super::prompt::Passage>, question: &str, marker: &str| super::prompt::AnswerInput {
        question: question.into(),
        passages,
        marker: marker.into(),
    };
    let m = super::prompt::marker(1);
    assert!(base().with_answer(input(vec![passage(3, "x")], "q?", &m)).is_ok());
    assert!(base().with_answer(input(vec![passage(9, "x")], "q?", &m)).is_err());
    assert!(base().with_answer(input(vec![passage(0, "x")], "q?", &m)).is_err());
    assert!(base().with_answer(input(vec![passage(3, "x")], " ", &m)).is_err());
    assert!(base().with_answer(input(vec![passage(3, "x")], &"q".repeat(2_001), &m)).is_err());
    assert!(base().with_answer(input(vec![passage(3, "x")], "q?", "")).is_err());
    assert!(base().with_answer(input(vec![passage(3, "x")], "q?", &"M".repeat(65))).is_err());
    assert!(base().with_answer(input(vec![passage(3, &"x".repeat(MAX_PROMPT_BYTES + 1))], "q?", &m)).is_err());
    assert!(base().with_answer(input((0..=MAX_SOURCES).map(|_| passage(3, "x")).collect(), "q?", &m)).is_err());
    let narrow = ModelRequest::new(AiKind::Narrow, vec![user("q")], vec![3], false, None, None, None).unwrap();
    assert!(narrow.with_answer(input(vec![passage(3, "x")], "q?", &m)).is_err());
    // Mail: never in a log.
    let shown = format!("{:?}", base().with_answer(input(vec![passage(3, "SECRET-PASSAGE")], "SECRET-QUESTION?", &m)).unwrap());
    assert!(!shown.contains("SECRET") && shown.contains("question_bytes"), "{shown}");
}

proptest::proptest! {
    #[test]
    fn any_end_of_an_answer_as_json_keeps_the_bounds(json in "\\PC{0,400}", sources in proptest::collection::vec(1u64..10, 0..6)) {
        let mut reqs = Requests::default();
        let id = reqs.start(answer_request(vec![3, 4, 5], 100_000)).unwrap().id;
        let _ = reqs.apply(id, ModelEvent::Done { text: Some(format!("{{{json}")), answered_by: None, sources: Some(sources) });
        let t = reqs.get(id).unwrap();
        proptest::prop_assert!(t.text.len() <= MAX_RESPONSE_BYTES);
        proptest::prop_assert!(t.cited.as_ref().is_none_or(|c| c.len() <= 3 && c.iter().all(|n| (1..=t.sources().len()).contains(n))));
    }
}
