// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::{Address, Message, ThreadIndex};

use super::corpus_tests::corpus;
use super::graph::*;

fn graph() -> (MailGraph, super::corpus_tests::Ids) {
    let (s, ids) = corpus();
    let threads = s.threads();
    (MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads), ids)
}

#[test]
fn a_message_is_linked_to_its_conversation_people_and_folder() {
    let (g, ids) = graph();
    let edges = g.edges_of(ids.reply);
    assert!(edges.contains(&Edge { kind: EdgeKind::Thread, to: Node::Thread(ids.budget) }), "{edges:?}");
    assert!(edges.contains(&Edge { kind: EdgeKind::From, to: Node::Person("kit@example.org".into()) }));
    assert!(edges.contains(&Edge { kind: EdgeKind::To, to: Node::Person("robin@example.org".into()) }));
    assert!(edges.contains(&Edge { kind: EdgeKind::Folder, to: Node::Folder(ids.inbox) }));
    assert!(g.edges_of(u64::MAX).is_empty());
}

#[test]
fn a_conversation_holds_its_messages_and_only_them() {
    let (g, ids) = graph();
    assert_eq!(g.messages_of(EdgeKind::Thread, &Node::Thread(ids.budget)), vec![ids.budget, ids.reply]);
    // A message linked to nothing is a conversation of its own.
    assert_eq!(g.messages_of(EdgeKind::Thread, &Node::Thread(ids.cabin)), vec![ids.cabin]);
}

#[test]
fn a_person_finds_the_messages_from_them_and_to_them() {
    let (g, ids) = graph();
    let ada = Node::Person("ada@example.org".into());
    assert_eq!(g.messages_of(EdgeKind::From, &ada), {
        let mut v = vec![ids.budget, ids.cabin, ids.archived];
        v.sort_unstable();
        v
    });
    assert!(g.messages_of(EdgeKind::To, &ada).is_empty());
    assert_eq!(g.messages_of(EdgeKind::To, &Node::Person("robin@example.org".into())).len(), g.len());
    assert_eq!(g.messages_of(EdgeKind::Folder, &Node::Folder(ids.archive)), vec![ids.archived]);
}

#[test]
fn people_are_named_by_address_or_by_words_of_a_name() {
    let (g, _) = graph();
    assert_eq!(g.people_named(" ADA@example.org "), ["ada@example.org"]);
    assert_eq!(g.people_named("Ada"), ["ada@example.org"]);
    assert_eq!(g.people_named("kit tanaka"), ["kit@example.org"]);
    assert_eq!(g.people_named("harbor hall"), ["events@harborhall.example"]);
    assert!(g.people_named("nobody@example.org").is_empty());
    assert!(g.people_named("Zebedee").is_empty());
    assert!(g.people_named("--").is_empty());
}

#[test]
fn hostile_addresses_and_recipient_lists_are_bounded() {
    let long = format!("{}@example.org", "x".repeat(1_000));
    let m = Message {
        id: 1,
        folder: 1,
        from: Address::new("", &long),
        to: (0..1_000).map(|i| Address::new("", format!("p{i}@example.org"))).collect(),
        cc: vec![Address::new("", "   "), Address::new("\u{0}", "\r\n@example.org")],
        ..Default::default()
    };
    let g = MailGraph::build([&m], &ThreadIndex::build([&m]));
    let edges = g.edges_of(1);
    assert!(!edges.iter().any(|e| e.kind == EdgeKind::From), "an address too long is no person");
    assert_eq!(edges.iter().filter(|e| e.kind == EdgeKind::To).count(), MAX_RECIPIENTS);
    assert!(!edges.iter().any(|e| e.to == Node::Person(String::new())));
}
