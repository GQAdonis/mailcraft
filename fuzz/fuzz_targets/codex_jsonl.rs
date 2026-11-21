// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Codex's `exec --json` stream (B6, 16.10) on the shared harness (B4, `contain::drive`): any
//! bytes, cut into lines whole and in two, judged as a run judges them. Never a panic; where the
//! stream is cut changes nothing; once a run is killed or answered nothing more comes of it; kill
//! kinds are never empty; reading a failure's words never panics.
#![no_main]

use mailcraft_ai::runtime::codex::{CodexDetector, classify};
use mailcraft_ai::runtime::contain::{Step, drive};

fn check(steps: &[Step]) {
    let mut ended = false;
    for s in steps {
        if ended {
            assert_eq!(s, &Step::Continue, "something came of a run after its end");
        }
        match s {
            Step::Kill(kind) => {
                assert!(!kind.is_empty());
                ended = true;
            }
            Step::TooLarge | Step::Done(_) => ended = true,
            // Codex's detector never holds the prompt.
            Step::WritePrompt => panic!("the Codex detector has no init gate"),
            Step::Continue => {}
        }
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let cut = data.first().map_or(0, |b| usize::from(*b));
    let whole = drive(CodexDetector::new(), data, 0);
    let split = drive(CodexDetector::new(), data, cut);
    assert_eq!(whole, split, "where the stream was cut changed what was judged");
    check(&whole);
    let _ = classify(&String::from_utf8_lossy(data));
});
