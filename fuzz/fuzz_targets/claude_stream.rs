// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Claude Code's stream-json detector (B5, 15.10) on B4's shared harness: any bytes, cut into
//! lines whole and cut in two, judged as a run judges them. Never a panic; where the stream is
//! cut changes nothing; once a run ends nothing more comes of it; kill kinds are never empty;
//! under the gate no answer comes before the init event was checked.
#![no_main]

use mailcraft_ai::runtime::claude_code::ClaudeStreamDetector;
use mailcraft_ai::runtime::contain::{Gate, Step, drive};

fn check(steps: &[Step], gate: Gate) {
    let mut ended = false;
    let mut init = false;
    for s in steps {
        if ended {
            assert_eq!(s, &Step::Continue, "something came of a run after its end");
        }
        match s {
            Step::Kill(kind) => {
                assert!(!kind.is_empty());
                ended = true;
            }
            Step::Done(_) => {
                assert!(gate == Gate::Now || init, "an answer before the init was checked");
                ended = true;
            }
            Step::TooLarge => ended = true,
            Step::WritePrompt => init = true,
            Step::Continue => {}
        }
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let cut = data.first().map_or(0, |b| usize::from(*b));
    for gate in [Gate::Now, Gate::AfterInit] {
        let whole = drive(ClaudeStreamDetector::new(gate), data, 0);
        let split = drive(ClaudeStreamDetector::new(gate), data, cut);
        assert_eq!(whole, split, "where the stream was cut changed what was judged");
        check(&whole, gate);
    }
});
