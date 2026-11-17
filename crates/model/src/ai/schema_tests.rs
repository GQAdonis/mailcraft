// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::narrow::{MAX_PROPOSAL_CHARS, NARROW_SCHEMA, check_query};
use super::schema::*;

#[test]
fn every_schema_the_product_sends_keeps_to_the_portable_subset() {
    assert!(!PRODUCT_SCHEMAS.is_empty());
    for (what, schema) in PRODUCT_SCHEMAS {
        assert_eq!(portable_problems(schema), Vec::<String>::new(), "the {what} schema");
    }
}

#[test]
fn the_narrowing_schema_has_no_max_length_and_the_length_is_held_in_code() {
    assert!(!NARROW_SCHEMA.contains("maxLength"));
    assert!(PRODUCT_SCHEMAS.iter().any(|(_, s)| *s == NARROW_SCHEMA));
    assert!(check_query(&"a".repeat(MAX_PROPOSAL_CHARS)).is_ok());
    assert!(check_query(&"a".repeat(MAX_PROPOSAL_CHARS + 1)).is_err());
}

#[test]
fn keywords_outside_the_subset_are_named() {
    for (schema, keyword) in [
        (r#"{"type":"object","properties":{"q":{"type":"string","maxLength":500}},"required":["q"],"additionalProperties":false}"#, "maxLength"),
        (r#"{"type":"object","properties":{"q":{"type":"string","minLength":1}},"required":["q"],"additionalProperties":false}"#, "minLength"),
        (r#"{"type":"object","properties":{"q":{"type":"string","pattern":"^a"}},"required":["q"],"additionalProperties":false}"#, "pattern"),
        (r#"{"type":"object","properties":{"q":{"type":"string","format":"email"}},"required":["q"],"additionalProperties":false}"#, "format"),
        (r#"{"type":"object","properties":{"n":{"type":"number","multipleOf":2}},"required":["n"],"additionalProperties":false}"#, "multipleOf"),
        (r##"{"type":"object","properties":{"q":{"$ref":"#/x","type":"string"}},"required":["q"],"additionalProperties":false}"##, "$ref"),
        (r#"{"type":"object","properties":{},"required":[],"additionalProperties":false,"anyOf":[]}"#, "anyOf"),
        (r#"{"type":"object","properties":{"n":{"type":"integer","enum":["a"]}},"required":["n"],"additionalProperties":false}"#, "enum"),
        (r#"{"type":"object","properties":{"q":{"type":"string","minimum":1}},"required":["q"],"additionalProperties":false}"#, "minimum"),
    ] {
        let problems = portable_problems(schema);
        assert!(problems.iter().any(|p| p.contains(keyword)), "{keyword}: {problems:?}");
    }
}

#[test]
fn the_subset_itself_is_taken() {
    let schema = r#"{
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "tone": {"type": "string", "enum": ["plain", "brief"]},
            "score": {"type": "number", "minimum": 0, "maximum": 1},
            "count": {"type": "integer", "minimum": 0},
            "sure": {"type": "boolean"},
            "cited": {"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 0, "maxItems": 3}
        },
        "required": ["answer", "tone", "score", "count", "sure", "cited"],
        "additionalProperties": false
    }"#;
    assert_eq!(portable_problems(schema), Vec::<String>::new());
}

#[test]
fn objects_are_closed_and_need_every_property() {
    let open = r#"{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}"#;
    assert!(portable_problems(open).iter().any(|p| p.contains("additionalProperties")));
    let loose = r#"{"type":"object","properties":{"q":{"type":"string"}},"required":["q"],"additionalProperties":true}"#;
    assert!(!portable_problems(loose).is_empty());
    let optional = r#"{"type":"object","properties":{"q":{"type":"string"},"r":{"type":"string"}},"required":["q"],"additionalProperties":false}"#;
    assert!(portable_problems(optional).iter().any(|p| p.contains("`r`")));
    let ghost = r#"{"type":"object","properties":{},"required":["q"],"additionalProperties":false}"#;
    assert!(portable_problems(ghost).iter().any(|p| p.contains("`q`")));
}

#[test]
fn types_are_one_name_of_the_subset_and_the_root_an_object() {
    assert!(!portable_problems(r#"{"type":"string"}"#).is_empty());
    assert!(!portable_problems(r#"{"type":["object","null"],"properties":{},"required":[],"additionalProperties":false}"#).is_empty());
    let null = r#"{"type":"object","properties":{"q":{"type":"null"}},"required":["q"],"additionalProperties":false}"#;
    assert!(!portable_problems(null).is_empty());
    assert_eq!(portable_problems("not json"), ["the schema isn't JSON"]);
    assert!(!portable_problems("[]").is_empty());
}

#[test]
fn hostile_schemas_are_bounded() {
    // Nested far past the depth cap.
    let mut deep = String::from(r#"{"type":"string"}"#);
    for _ in 0..100 {
        deep = format!(r#"{{"type":"array","items":{deep}}}"#);
    }
    let deep = format!(r#"{{"type":"object","properties":{{"a":{deep}}},"required":["a"],"additionalProperties":false}}"#);
    let problems = portable_problems(&deep);
    assert!(problems.iter().any(|p| p.contains("deeper")), "{problems:?}");
    // Very many properties, each wrong: the problems are capped.
    let props: Vec<String> = (0..10_000).map(|i| format!(r#""p{i}":{{"type":"string","pattern":"x"}}"#)).collect();
    let wide = format!(r#"{{"type":"object","properties":{{{}}},"required":[],"additionalProperties":false}}"#, props.join(","));
    assert!(portable_problems(&wide).len() <= 64);
}

proptest::proptest! {
    #[test]
    fn any_text_is_linted_without_panicking(text in "\\PC{0,400}") {
        let _ = portable_problems(&text);
    }
}
