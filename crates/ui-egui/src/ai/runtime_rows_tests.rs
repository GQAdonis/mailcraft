// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime chain's words (B11): the status chip reads the chain (SCN-RT-CHIP-001), the
//! narrowing badge names the runtime that would answer (SCN-RT-BADGE-001), each row says its
//! state in the page's words only, a move keeps the hidden runtimes where they were, and the
//! consent dialog says what is sent, to whom, what it may do and how to stop.

use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, Where};
use serde_json::json;

use super::narrow::Route;
use super::runtime_consent::{columns, consent_text};
use super::runtime_rows::{Action, CompatFacts, Extra, Facts, Platform, Reason, RowFacts, Status};
use super::status::{ChainChip, TYPED_STATE};
use crate::control::{RuntimeSettings, RuntimeState, RuntimeStatus};

const MAC: Platform = Platform { mac: true, windows: false };

fn row(id: &str, enabled: bool, state: RuntimeState, place: Where) -> RowFacts {
    RowFacts {
        id: RuntimeId::parse(id).unwrap(),
        enabled,
        status: RuntimeStatus { state, ..Default::default() },
        place,
        extra: Extra::default(),
        compat: None,
        model: None,
    }
}

fn facts(rows: Vec<RowFacts>, consents: &[(&str, &str)]) -> Facts {
    let chain = Chain::new(rows.iter().map(|r| ChainEntry::new(r.id.clone()).with_enabled(r.enabled))).unwrap();
    let mut c = Consents::new();
    for (account, runtime) in consents {
        c = c.with(account, &RuntimeId::parse(runtime).unwrap(), true).unwrap();
    }
    Facts { settings: RuntimeSettings { chain, consents: c, fallback: Fallback::Next }, rows, platform: Some(MAC), signing_in: None }
}

#[test]
fn scn_rt_chip_001_chip_states() {
    use RuntimeState::Ready;
    let none = facts(vec![row("on_device", false, Ready, Where::Local), row("claude_code", false, Ready, Where::Leaves)], &[]);
    assert_eq!(ChainChip::of(&none), ChainChip::Off);
    assert_eq!(ChainChip::of(&none).text(), "AI off");
    let local = facts(vec![row("on_device", true, Ready, Where::Local), row("claude_code", false, Ready, Where::Leaves)], &[]);
    assert_eq!(ChainChip::of(&local).text(), "AI: on this computer");
    let remote = facts(
        vec![row("on_device", true, Ready, Where::Local), row("claude_code", true, Ready, Where::Leaves)],
        &[("ada@example.org", "claude_code")],
    );
    let chip = ChainChip::of(&remote);
    assert_eq!(chip.text(), "AI: may leave for ada@example.org");
    assert_eq!(chip.icon(), "aiRemote");
    assert!(chip.tip().contains("ada@example.org") && chip.tip().contains("Claude Code") && chip.tip().contains("AI assistance"), "{}", chip.tip());
    // Two accounts are counted.
    let two = facts(
        vec![row("claude_code", true, Ready, Where::Leaves), row("codex", true, Ready, Where::Leaves)],
        &[("ada@example.org", "claude_code"), ("team@lumen.example", "codex")],
    );
    assert_eq!(ChainChip::of(&two).text(), "AI: may leave for 2 accounts");
}

/// SCN-RT-CHIP-001, second case. The spec says "AI: on this computer"; the design brief (§11.1,
/// Q1) carries the owner's 2026-11-17 answer over and says where typed searches may go, because
/// Find by Description's words do go there without any account's yes. Until the amendment is
/// signed the brief's state is built; `TYPED_STATE` folds it back into "on this computer".
#[test]
fn scn_rt_chip_001_leaves_runtime_without_consent() {
    let f = facts(vec![row("on_device", true, RuntimeState::Ready, Where::Local), row("codex", true, RuntimeState::Ready, Where::Leaves)], &[]);
    // A consent for a runtime that is off, or for one that isn't in the chain, counts for nothing.
    let off = facts(
        vec![
            row("on_device", true, RuntimeState::Ready, Where::Local),
            row("codex", true, RuntimeState::Ready, Where::Leaves),
            row("chatgpt", false, RuntimeState::Ready, Where::Leaves),
        ],
        &[("ada@example.org", "chatgpt")],
    );
    for f in [f, off] {
        let chip = ChainChip::of(&f);
        if TYPED_STATE {
            assert_eq!(chip.text(), "AI: your typed searches may go to Codex");
            assert!(chip.tip().contains("No mail is sent there until you allow an account"), "{}", chip.tip());
        } else {
            assert_eq!(chip.text(), "AI: on this computer");
        }
        assert_ne!(chip.text(), "AI off");
    }
}

#[test]
fn the_summary_and_the_chip_never_disagree() {
    let f = facts(vec![row("claude_code", true, RuntimeState::Ready, Where::Leaves)], &[("ada@example.org", "claude_code")]);
    let (words, local, _) = super::settings_runtimes::where_summary(&f);
    assert!(!local);
    assert_eq!(words, "Mail may leave: ada@example.org → Claude Code");
    let quiet = facts(vec![row("on_device", true, RuntimeState::Ready, Where::Local)], &[]);
    assert_eq!(super::settings_runtimes::where_summary(&quiet), ("Nothing leaves this computer".to_string(), true, None));
    let many = facts(
        vec![row("claude_code", true, RuntimeState::Ready, Where::Leaves), row("codex", true, RuntimeState::Ready, Where::Leaves)],
        &[("a@example.org", "claude_code"), ("b@example.org", "claude_code"), ("c@example.org", "codex")],
    );
    let (words, _, tip) = super::settings_runtimes::where_summary(&many);
    assert!(words.ends_with("and 1 more"), "{words}");
    assert!(tip.is_some_and(|t| t.contains("c@example.org → Codex")));
}

#[test]
fn scn_rt_badge_001_badge_per_runtime_type() {
    let mut compat = row("compat:lab", true, RuntimeState::Ready, Where::Leaves);
    compat.compat = Some(CompatFacts { kind: "remote".into(), base_url: "https://user:pw@api.example.org/v1?k=1".into() });
    let cases = [
        (row("on_device", true, RuntimeState::Ready, Where::Local), "On this computer", true),
        (compat, "lab", false),
        (row("chatgpt", true, RuntimeState::Ready, Where::Leaves), "ChatGPT plan", false),
        (row("claude_code", true, RuntimeState::Ready, Where::Leaves), "Claude Code", false),
        (row("codex", true, RuntimeState::Ready, Where::Leaves), "Codex", false),
        (row("uar", true, RuntimeState::Ready, Where::Leaves), "UAR", false),
    ];
    for (first, words, local) in cases {
        // An earlier runtime that is off or can't answer is passed over.
        let before =
            vec![row("chatgpt", false, RuntimeState::Ready, Where::Leaves), row("claude_code", true, RuntimeState::SignedOut, Where::Leaves)];
        let id = first.id.clone();
        let rows: Vec<RowFacts> = before.into_iter().filter(|r| r.id != id).chain([first]).collect();
        let route = Route::of_chain(&facts(rows, &[])).expect("something answers");
        assert_eq!(route.words(), words);
        let tip = route.tip().expect("a tip");
        if local {
            assert!(tip.contains("stays on this computer"), "{tip}");
        } else {
            assert!(tip.contains("No mail is sent") && !tip.contains("pw"), "{tip}");
        }
    }
    let nothing = facts(vec![row("codex", true, RuntimeState::NotFound, Where::Leaves)], &[]);
    assert_eq!(Route::of_chain(&nothing), None);
}

#[test]
fn rows_say_their_state_in_the_pages_words() {
    let f = facts(
        vec![
            row("claude_code", true, RuntimeState::SignedOut, Where::Leaves),
            row("codex", false, RuntimeState::Ready, Where::Leaves),
            row("codex", true, RuntimeState::Ready, Where::Leaves),
        ]
        .into_iter()
        .take(2)
        .collect(),
        &[],
    );
    let rows = f.rows();
    assert_eq!((rows[0].status.clone(), rows[0].action), (Status::SignedOut, Some(Action::SignIn)));
    assert!(rows[0].line2.contains("claude auth login"));
    assert_eq!(rows[0].place.words, "Sends to Anthropic");
    assert_eq!(rows[1].status, Status::Off, "Codex found and off");
    let on = facts(vec![row("codex", true, RuntimeState::Ready, Where::Leaves)], &[]);
    assert_eq!(on.rows()[0].status, Status::SignInNotChecked);
    let signed_out = facts(vec![row("codex", true, RuntimeState::SignedOut, Where::Leaves)], &[]);
    let r = &signed_out.rows()[0];
    assert_eq!(r.action, Some(Action::LookAgain), "Codex is never signed in by MailCraft");
    assert!(r.line2.contains("MailCraft never runs it"));
    for state in [RuntimeState::NotFound, RuntimeState::Unavailable] {
        let f = facts(vec![row("claude_code", true, state, Where::Leaves)], &[]);
        assert!(!f.rows()[0].status.switchable(), "{state:?}");
    }
    // No word anywhere says a plan was declined, or asks for a key for a CLI.
    let all = [
        Status::Ready,
        Status::SignInNotChecked,
        Status::Off,
        Status::SignedOut,
        Status::SignInExpired,
        Status::SignInEnded,
        Status::SigningIn,
        Status::NeedsKey,
        Status::NotInstalled,
        Status::NotRunning,
        Status::Running,
        Status::NotTurnedOn,
        Status::Looking,
        Status::NotCheckedYet,
        Status::CantRunHere,
        Status::Broken(Reason::Unsaid),
    ];
    for s in all {
        let w = s.words().to_lowercase();
        assert!(!w.contains("subscription") && !w.contains("declined") && !w.contains("api key"), "{w}");
        let (_, icon) = s.look();
        assert!(icon.is_some() || !w.is_empty(), "never colour alone");
    }
}

#[test]
fn the_host_s_own_words_win_and_are_read_as_hostile() {
    let mut r = row("claude_code", true, RuntimeState::Ready, Where::Leaves);
    r.extra = Extra::read(&json!({"status": "broken", "reason": "othersWriteFolder", "folder": "/opt/homebrew/bin\u{0}\n", "usesPlan": "yes"}));
    let f = facts(vec![r], &[]);
    let shown = &f.rows()[0];
    assert_eq!(shown.status, Status::Broken(Reason::OthersWriteFolder(Some("/opt/homebrew/bin".into()))));
    assert!(shown.line2.contains("other users can change its folder"));
    if let Status::Broken(reason) = &shown.status {
        assert!(reason.remedy().contains("/opt/homebrew/bin"));
    }
    for junk in [json!(null), json!([1]), json!({"status": 7, "email": {"a": 1}, "plan": "x".repeat(10_000)})] {
        let e = Extra::read(&junk);
        assert!(e.status.is_none() && e.email.is_none());
        assert!(e.plan.as_ref().is_none_or(|p| p.chars().count() <= 160));
    }
    let mut plan = row("claude_code", true, RuntimeState::Ready, Where::Leaves);
    plan.extra.uses_plan = Some(true);
    assert_eq!(facts(vec![plan], &[]).rows()[0].line2, "Uses your Claude plan");
}

#[test]
fn hidden_runtimes_keep_their_place_when_another_moves() {
    let windows = Platform { mac: false, windows: true };
    let mut f = facts(
        vec![
            row("on_device", true, RuntimeState::Ready, Where::Local),
            row("chatgpt", true, RuntimeState::Ready, Where::Leaves),
            row("codex", false, RuntimeState::Ready, Where::Leaves),
            row("claude_code", true, RuntimeState::Ready, Where::Leaves),
        ],
        &[],
    );
    f.platform = Some(windows);
    let shown: Vec<String> = f.rows().iter().map(|r| r.id.to_string()).collect();
    assert_eq!(shown, ["chatgpt", "claude_code"], "no on-device model off a Mac, no Codex on Windows");
    let order: Vec<String> = f.moved(0, true).unwrap().iter().map(|r| r.to_string()).collect();
    assert_eq!(order, ["on_device", "claude_code", "codex", "chatgpt"]);
    assert_eq!(f.moved(0, false), None);
    assert_eq!(f.moved(1, true), None);
    assert_eq!(f.moved(usize::MAX, true), None);
}

#[test]
fn the_matrix_has_one_local_column_then_each_runtime_that_leaves() {
    let f = facts(
        vec![
            row("on_device", true, RuntimeState::Ready, Where::Local),
            row("chatgpt", false, RuntimeState::SignedOut, Where::Leaves),
            row("uar", false, RuntimeState::NotFound, Where::Leaves),
            row("claude_code", true, RuntimeState::Ready, Where::Leaves),
        ],
        &[],
    );
    let names: Vec<(String, bool)> = columns(&f).into_iter().map(|c| (c.name, c.off)).collect();
    assert_eq!(names, [("On this computer".into(), false), ("ChatGPT plan".into(), true), ("Claude Code".into(), false)]);
}

#[test]
fn the_consent_dialog_says_what_whom_what_it_may_do_and_how_to_stop() {
    for id in ["chatgpt", "claude_code", "codex", "uar"] {
        let r = row(id, true, RuntimeState::Ready, Where::Leaves);
        let (title, parts) = consent_text(&r, "team@lumen.example");
        assert!(title.contains("team@lumen.example"), "{title}");
        let leads: Vec<&str> = parts.iter().map(|(lead, _)| lead.as_str()).filter(|l| !l.is_empty()).collect();
        assert_eq!(leads, ["What is sent.", "Who receives it.", "What it may do.", "How to stop."], "{id}");
        let all: String = parts.iter().map(|(_, w)| w.as_str()).collect::<Vec<_>>().join(" ");
        assert!(all.contains("never sent") && all.contains("can't be called back"), "{id}");
        if id == "codex" {
            assert!(all.contains("skills and agent instructions") && all.contains("sandbox"), "the Codex caveat");
        }
        if id == "chatgpt" || id == "claude_code" || id == "codex" {
            assert!(all.contains("usage limits"), "{id}: plan quota");
        }
    }
}
