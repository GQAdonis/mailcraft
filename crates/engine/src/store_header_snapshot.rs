// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A projection of the persisted records, before a normal open repairs the mailbox. Never
//! opens sidecars, migrates a legacy file, fills bodies or consumes a repaired read-ahead cache.
use super::{HeaderWire, MAX_INDEX, SPLIT_VERSION, index, open_budget, parse_ref};
use mailcraft_model::load_limit::{self, Budget};
use mailcraft_model::{
    Account, AccountKind, Folder, FolderKind, HeaderBlock, Id, MAX_ACCOUNTS, MAX_FOLDER_DEPTH, MAX_FOLDERS, MAX_ID, Mailbox, Message,
};
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeSeed, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::Arc,
};

const INVALID: &str = "invalid_snapshot_provenance";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedAccount {
    id: Id,
    kind: AccountKind,
    shared_of: Option<Id>,
}

/// Only the fields consumed by the evidence projection. Skipped bodies and attachments never
/// enter the snapshot, including when a legacy single-file store is streamed.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SavedMessage {
    id: Id,
    account: Id,
    folder: Id,
    subject: String,
    date: i64,
    received: i64,
    draft: bool,
    message_id: String,
    in_reply_to: String,
    references: Vec<String>,
    server: Option<mailcraft_model::sync::ServerMessage>,
    headers: Vec<(String, String)>,
    header_block: HeaderBlock,
}

impl From<SavedMessage> for Message {
    fn from(m: SavedMessage) -> Self {
        Self {
            id: m.id,
            account: m.account,
            folder: m.folder,
            subject: m.subject,
            date: m.date,
            received: m.received,
            draft: m.draft,
            message_id: m.message_id,
            in_reply_to: m.in_reply_to,
            references: m.references,
            server: m.server,
            headers: m.headers,
            header_block: m.header_block,
            ..Self::default()
        }
    }
}

#[derive(Deserialize)]
struct HeaderLocation {
    headers: Option<HeaderWire>,
}

/// Duplicate keys (including two spellings of one numeric ID) must never silently replace
/// the original message or its header-block location.
fn unique<'de, D: Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<BTreeMap<Id, T>, D::Error> {
    struct ById<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for ById<T> {
        type Value = BTreeMap<Id, T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("unique numeric record IDs")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((id, entry)) = map.next_entry()? {
                if out.insert(id, entry).is_some() {
                    return Err(serde::de::Error::custom(INVALID));
                }
            }
            Ok(out)
        }
    }
    de.deserialize_map(ById(std::marker::PhantomData))
}

#[derive(Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    accounts: Vec<SavedAccount>,
    folders: Vec<Folder>,
    #[serde(default, deserialize_with = "unique")]
    messages: BTreeMap<Id, SavedMessage>,
    #[serde(default, deserialize_with = "unique")]
    blobs: BTreeMap<Id, HeaderLocation>,
}

impl Saved {
    fn mailbox(mut self) -> Result<Mailbox, String> {
        let valid = |id| (1..=MAX_ID).contains(&id);
        let accounts: HashSet<_> = self.accounts.iter().map(|a| a.id).collect();
        if accounts.len() != self.accounts.len()
            || accounts.len() > MAX_ACCOUNTS
            || !accounts.iter().all(|id| valid(*id))
            || self.folders.len() > MAX_FOLDERS
        {
            return Err(INVALID.into());
        }
        let owners: HashSet<_> = self.accounts.iter().filter(|a| a.shared_of.is_none()).map(|a| a.id).collect();
        if self.accounts.iter().any(|a| a.shared_of.is_some_and(|owner| !owners.contains(&owner))) {
            return Err(INVALID.into());
        }
        let folders: HashMap<_, _> = self.folders.iter().map(|f| (f.id, f)).collect();
        let mut roles = HashSet::new();
        if folders.len() != self.folders.len() {
            return Err(INVALID.into());
        }
        for f in &self.folders {
            if !valid(f.id)
                || accounts.contains(&f.id)
                || !accounts.contains(&f.account)
                || (f.kind != FolderKind::Custom && (f.parent.is_some() || !roles.insert((f.account, f.kind))))
            {
                return Err(INVALID.into());
            }
            let mut parent = f.parent;
            let mut depth = 0;
            while let Some(id) = parent {
                let p = folders.get(&id).ok_or(INVALID)?;
                depth += 1;
                if p.account != f.account || depth > MAX_FOLDER_DEPTH {
                    return Err(INVALID.into());
                }
                parent = p.parent;
            }
        }
        let mut mailbox = Mailbox::default();
        for (id, raw) in self.messages {
            if !valid(id) || raw.id != id || !folders.get(&raw.folder).is_some_and(|f| f.account == raw.account) {
                return Err(INVALID.into());
            }
            let mut msg = Message::from(raw);
            if let Some(h) = self.blobs.remove(&id).and_then(|b| b.headers) {
                // An external header block may carry Auto-Submitted. Keep its unknown state
                // without ever opening or creating a sidecar.
                if !msg.header_block.data.is_empty() {
                    return Err(INVALID.into());
                }
                msg.header_block.kept = Some(parse_ref(&h.name()).ok_or(INVALID)?.to_model());
            }
            mailbox.messages.insert(id, Arc::new(msg));
        }
        if !self.blobs.is_empty() {
            return Err(INVALID.into());
        }
        mailbox.accounts =
            self.accounts.into_iter().map(|a| Account { id: a.id, kind: a.kind, shared_of: a.shared_of, ..Default::default() }).collect();
        mailbox.folders = self.folders;
        Ok(mailbox)
    }
}

fn indexed(reader: impl Read, len: u64, budget: &Arc<Budget>) -> Result<Saved, String> {
    let replay = index::replay_within(reader, len, index::MAX_RECORDS, Some(budget)).map_err(|_| INVALID)?;
    if replay.tail != index::Tail::Clean || replay.lost_saves != 0 || replay.lost_records != 0 {
        return Err("incomplete_snapshot".into());
    }
    let (rest, _) = replay.rest.ok_or(INVALID)?;
    let mut saved: Saved = load_limit::read_json(&rest, budget).map_err(|_| INVALID)?;
    if !saved.messages.is_empty() || !saved.blobs.is_empty() {
        return Err(INVALID.into());
    }
    for ((collection, id), record) in replay.items {
        if collection == index::Coll::Message {
            let msg = load_limit::read_json(record.json(), budget).map_err(|_| INVALID)?;
            saved.messages.insert(id, msg);
            if !record.refs().is_empty() {
                let location = load_limit::read_json(record.refs(), budget).map_err(|_| INVALID)?;
                saved.blobs.insert(id, location);
            }
        }
        budget.give_back(record.cost());
    }
    Ok(saved)
}

pub(crate) fn read(path: &Path) -> Result<Mailbox, String> {
    let file = crate::io::regular_file(path).map_err(|_| "snapshot_unavailable")?;
    let initial = file.metadata().map_err(|_| "snapshot_unavailable")?;
    let len = initial.len();
    if len > MAX_INDEX {
        return Err("snapshot_too_large".into());
    }
    let mut reader = BufReader::new((&file).take(len));
    let prefix = reader.fill_buf().map_err(|_| "snapshot_unavailable")?;
    let budget = open_budget();
    let saved = if prefix.starts_with(crate::io::INDEX_MAGIC) {
        indexed(reader, len, &budget)?
    } else {
        if prefix.starts_with(&[0xef, 0xbb, 0xbf]) {
            reader.consume(3);
        }
        let mut de = serde_json::Deserializer::from_reader(reader);
        let saved = load_limit::within(&budget, || load_limit::Counted::<Saved>::default().deserialize(&mut de)).map_err(|_| INVALID)?;
        de.end().and_then(|()| load_limit::refused_within(&budget)).map_err(|_| INVALID)?;
        if saved.version > SPLIT_VERSION || (saved.version < SPLIT_VERSION && !saved.blobs.is_empty()) {
            return Err(INVALID.into());
        }
        saved
    };
    let after = file.metadata().map_err(|_| "snapshot_unavailable")?;
    if len != after.len() || initial.modified().ok() != after.modified().ok() {
        return Err("stale_snapshot".into());
    }
    saved.mailbox()
}
