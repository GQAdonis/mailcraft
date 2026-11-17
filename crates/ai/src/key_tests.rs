// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A provider's key: what is taken as one, and that it is never shown, in `Debug` or an error.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::error::AiError;
use crate::key::{ApiKey, MAX_KEY};

const KEY: &str = "sk-test-not-a-real-key-0123456789";

#[test]
fn debug_never_shows_the_key() {
    let key = ApiKey::new(KEY).unwrap();
    assert_eq!(format!("{key:?}"), "ApiKey(***)");
    assert_eq!(format!("{:?}", Some(key.clone())), "Some(ApiKey(***))");
    assert_eq!(format!("{key:#?}"), "ApiKey(***)");
    assert_eq!(key.expose(), KEY);
}

#[test]
fn a_key_is_printable_and_without_spaces_and_the_error_never_repeats_it() {
    assert_eq!(ApiKey::new(&format!("  {KEY}\n")).unwrap().expose(), KEY);
    for bad in ["", "   ", "sk with space", "sk-\r\nX-Injected: 1", "sk-\0", "sk-é", &"k".repeat(MAX_KEY + 1)] {
        let error = ApiKey::new(bad).unwrap_err();
        assert!(matches!(error, AiError::Config(_)), "{bad:?}");
        let said = format!("{error} {error:?}");
        assert!(bad.trim().is_empty() || !said.contains(bad.trim()), "{said}");
    }
    assert!(ApiKey::new(&"k".repeat(MAX_KEY)).is_ok());
}
