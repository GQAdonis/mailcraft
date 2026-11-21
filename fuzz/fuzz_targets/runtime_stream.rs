// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared harness for every runtime's stream detector (B4, 14.11): any bytes, cut into lines
//! (whole and cut in two) and judged as a run judges them. Never a panic; where the stream is cut
//! changes nothing; once a run is killed nothing more comes of it and no answer is handed back;
//! kinds are never empty. This target drives a detector whose verdicts the bytes choose, so every
//! path of the splitter and the judge is reached; B5, B6 and B9 each add a target that calls
//! `contain::drive` with their own parser.
#![no_main]

use mailcraft_ai::runtime::contain::{Gate, Step, StreamDetector, Verdict, drive};

/// A detector whose verdict is chosen by the line's first byte.
struct ByteChosen(Gate);

impl StreamDetector for ByteChosen {
    fn gate(&self) -> Gate {
        self.0
    }
    fn line(&mut self, bytes: &[u8]) -> Verdict {
        let rest = String::from_utf8_lossy(bytes.get(1..).unwrap_or(&[])).into_owned();
        match bytes.first().map(|b| b % 6) {
            Some(0) => Verdict::Text(rest),
            Some(1) => Verdict::Final(rest),
            Some(2) => Verdict::Note("note"),
            Some(3) => Verdict::InitOk,
            Some(4) => Verdict::Text(String::new()),
            _ => Verdict::Kill("fuzz_kill"),
        }
    }
}

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
            Step::Continue | Step::WritePrompt => {}
        }
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let cut = data.first().map_or(0, |b| usize::from(*b));
    for gate in [Gate::Now, Gate::AfterInit] {
        let whole = drive(ByteChosen(gate), data, 0);
        let split = drive(ByteChosen(gate), data, cut);
        assert_eq!(whole, split, "where the stream was cut changed what was judged");
        check(&whole);
        if gate == Gate::AfterInit {
            // Under the gate no answer comes before the init event was checked.
            let first_answer = whole.iter().position(|s| matches!(s, Step::Done(_)));
            let first_init = whole.iter().position(|s| *s == Step::WritePrompt);
            if let Some(a) = first_answer {
                assert!(first_init.is_some_and(|i| i < a));
            }
        }
    }
});
