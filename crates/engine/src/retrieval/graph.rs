// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a message is linked to: its conversation, the people it is from, to and copied to, and
//! its folder. [`MailGraph`] works it out from the mailbox and its conversations
//! ([`mailcraft_model::ThreadIndex`]) and keeps it in memory, never on disk: it is built again
//! whenever it is wanted.
//!
//! A person is known by their address, in lower case; a conversation by its key (the smallest id
//! of its messages). Everything is read from mail, so it is bounded: at most [`MAX_RECIPIENTS`]
//! people a message is to and as many it is copied to, and addresses of at most
//! [`MAX_ADDRESS_CHARS`] characters.

use std::collections::{BTreeMap, HashMap};

use mailcraft_model::{Id, Message, ThreadIndex, words};

/// Most people of a message's `To` (and as many of its `Cc`) that are linked.
pub const MAX_RECIPIENTS: usize = 100;
/// Longest address linked, in characters.
pub const MAX_ADDRESS_CHARS: usize = 320;
/// Most people a name finds.
const MAX_NAMED: usize = 1_000;

/// Something a message is linked to, or the message itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Message(Id),
    /// A conversation, by its key.
    Thread(Id),
    /// A person, by their address in lower case.
    Person(String),
    Folder(Id),
}

/// How a message is linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    /// It is part of a conversation.
    Thread,
    /// It is from a person.
    From,
    /// It is to a person.
    To,
    /// A person is copied on it.
    Cc,
    /// It is in a folder.
    Folder,
}

/// One link of a message.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Edge {
    pub kind: EdgeKind,
    pub to: Node,
}

/// What messages are linked to.
pub trait GraphStore {
    /// The links of `message`; none for a message it doesn't know.
    fn edges_of(&self, message: Id) -> Vec<Edge>;
    /// The messages linked to `node` by `kind` (the messages of a conversation, from a person, to
    /// a person, in a folder), in order of id.
    fn messages_of(&self, kind: EdgeKind, node: &Node) -> Vec<Id>;
    /// The people `text` names: an address, or words of a name or an address (`ada`, `Ada
    /// Lovelace`), each at the start of a word of it, one after another.
    fn people_named(&self, text: &str) -> Vec<String>;
}

/// The links of the messages of a mailbox, in memory.
#[derive(Clone, Debug, Default)]
pub struct MailGraph {
    edges: HashMap<Id, Vec<Edge>>,
    by_node: HashMap<(EdgeKind, Node), Vec<Id>>,
    /// Every person, by address, with a name they were given (the first seen).
    people: BTreeMap<String, String>,
}

/// An address as a person is known by: trimmed, in lower case; `None` for none, or one too long.
fn person_key(email: &str) -> Option<String> {
    let email = email.trim();
    if email.is_empty() || email.chars().nth(MAX_ADDRESS_CHARS).is_some() {
        return None;
    }
    Some(email.to_lowercase())
}

impl MailGraph {
    /// The links of `messages`, whose conversations `threads` knows.
    pub fn build<'a>(messages: impl IntoIterator<Item = &'a Message>, threads: &ThreadIndex) -> MailGraph {
        let mut g = MailGraph::default();
        for m in messages {
            let thread = Edge { kind: EdgeKind::Thread, to: Node::Thread(threads.key(m.id).0) };
            let folder = Edge { kind: EdgeKind::Folder, to: Node::Folder(m.folder) };
            let mut edges = vec![thread, folder];
            let people = std::iter::once((EdgeKind::From, &m.from))
                .chain(m.to.iter().take(MAX_RECIPIENTS).map(|a| (EdgeKind::To, a)))
                .chain(m.cc.iter().take(MAX_RECIPIENTS).map(|a| (EdgeKind::Cc, a)));
            for (kind, address) in people {
                let Some(key) = person_key(&address.email) else { continue };
                let name = g.people.entry(key.clone()).or_default();
                if name.is_empty() {
                    name.clone_from(&address.name);
                }
                let edge = Edge { kind, to: Node::Person(key) };
                if !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
            for e in &edges {
                g.by_node.entry((e.kind, e.to.clone())).or_default().push(m.id);
            }
            g.edges.insert(m.id, edges);
        }
        for ids in g.by_node.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        g
    }

    /// How many messages it knows.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

impl GraphStore for MailGraph {
    fn edges_of(&self, message: Id) -> Vec<Edge> {
        self.edges.get(&message).cloned().unwrap_or_default()
    }

    fn messages_of(&self, kind: EdgeKind, node: &Node) -> Vec<Id> {
        self.by_node.get(&(kind, node.clone())).cloned().unwrap_or_default()
    }

    fn people_named(&self, text: &str) -> Vec<String> {
        let text = text.trim();
        if text.contains('@') {
            return person_key(text).filter(|k| self.people.contains_key(k)).into_iter().collect();
        }
        let wanted = words::words(text);
        if wanted.is_empty() {
            return Vec::new();
        }
        self.people
            .iter()
            .filter(|(address, name)| words::starts_in(name, &wanted) || words::starts_in(address, &wanted))
            .take(MAX_NAMED)
            .map(|(address, _)| address.clone())
            .collect()
    }
}
