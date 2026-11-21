// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `claude auth status --json` as the Claude Code adapter reads it (B5, 15.10): any bytes, never
//! a panic; over the cap is always broken; a usable status always had a `loggedIn` of true.
#![no_main]

use mailcraft_ai::runtime::claude_code::{AuthStatus, MAX_STATUS_BYTES, StatusProblem, parse_auth_status};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let status = parse_auth_status(data);
    if data.len() > MAX_STATUS_BYTES {
        assert_eq!(status, AuthStatus::Broken(StatusProblem::TooLarge));
    }
    if status.is_usable() || status == AuthStatus::NeedsSignIn {
        assert!(data.windows(8).any(|w| w == b"loggedIn"), "a status without loggedIn was read");
    }
});
