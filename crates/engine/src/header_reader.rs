// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A read-only, owner-scoped snapshot for paged MCP reads. The mailbox is projected once;
//! no command registry, body, network transport, or writable session is retained.
use std::collections::{BTreeMap, HashSet};

use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox, Message, ThreadIndex};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_ROWS: usize = 2_000;
const MAX_PAGE: usize = 100;

#[derive(Serialize)]
struct Row {
    id: Id,
    message_identity: Vec<u8>,
    folder: Id,
    thread: Option<Id>,
    timestamp: i64,
    direction: &'static str,
    subject: String,
    subject_truncated: bool,
}

struct Snapshot {
    rows: Vec<Row>,
    binding: Vec<u8>,
    inbox: bool,
    sent: bool,
}

/// Authority is supplied by the operator when the connector starts, never by a tool argument.
pub struct HeaderReader {
    account: Id,
    cutoff: i64,
    key: hmac::Key,
    views: BTreeMap<Vec<Id>, Snapshot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Query {
    account: Id,
    folders: Vec<Id>,
    cutoff: i64,
    fields: Vec<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    source_ref: Option<String>,
}

fn timestamp(m: &Message) -> i64 {
    if m.received > 0 { m.received } else { m.date }
}

fn direction(m: &Message, folder: FolderKind) -> &'static str {
    if m.draft {
        return "draft";
    }
    match folder {
        FolderKind::Inbox => "inbound",
        FolderKind::Sent => {
            let auto = |value: &str| !value.trim().eq_ignore_ascii_case("no") && !value.trim().is_empty();
            if m.header_values("Auto-Submitted").any(auto) {
                return "auto";
            }
            // Older saves may keep this header in their bounded extra-header block.
            if !m.header_block.is_loaded() {
                return "unknown";
            }
            if m.header_block.lines().iter().any(|h| h.name.eq_ignore_ascii_case("Auto-Submitted") && auto(&h.value)) { "auto" } else { "outbound" }
        }
        _ => "unknown",
    }
}

fn subject(value: &str) -> (String, bool) {
    let mut text = String::new();
    for c in value.chars() {
        if text.len() + c.len_utf8() > 256 {
            return (text, true);
        }
        if c.is_control() || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            text.push(' ');
        } else {
            text.push(c);
        }
    }
    (text, false)
}

impl HeaderReader {
    /// Snapshot the explicitly named folders of an already-opened mailbox. Only an inbox and a
    /// sent folder are supported by v1. Each subset has its own thread graph and handle namespace.
    pub fn new(mailbox: &Mailbox, account: Id, mut folders: Vec<Id>, cutoff: i64, identity: &str, secret: &[u8; 32]) -> Result<Self, &'static str> {
        folders.sort_unstable();
        if !(1..=2).contains(&folders.len()) || folders.windows(2).any(|w| w.first() == w.last()) {
            return Err("invalid_grant");
        }
        let owner = mailbox.accounts.iter().find(|a| a.id == account).ok_or("scope_denied")?;
        let mut kinds = BTreeMap::new();
        for id in &folders {
            let f = mailbox.folders.iter().find(|f| f.id == *id && f.account == account).ok_or("scope_denied")?;
            if !matches!(f.kind, FolderKind::Inbox | FolderKind::Sent) || kinds.values().any(|v| v == &f.kind) {
                return Err("invalid_grant");
            }
            kinds.insert(*id, f.kind);
        }
        let messages: Vec<&Message> = mailbox
            .messages
            .values()
            .map(|m| &**m)
            .filter(|m| m.account == account && kinds.contains_key(&m.folder) && timestamp(m) <= cutoff)
            .take(MAX_ROWS + 1)
            .collect();
        if messages.len() > MAX_ROWS {
            return Err("row_budget_exceeded");
        }
        let mut views = BTreeMap::new();
        let subsets = std::iter::once(folders.clone()).chain(folders.iter().filter(|_| folders.len() > 1).map(|id| vec![*id]));
        for subset in subsets {
            let mut selected: Vec<_> = messages.iter().copied().filter(|m| subset.contains(&m.folder)).collect();
            selected.sort_unstable_by_key(|m| (timestamp(m), m.id));
            let threads = ThreadIndex::evidence_keys(&selected);
            let mut rows = Vec::new();
            let mut seen = HashSet::new();
            for m in selected {
                let folder = *kinds.get(&m.folder).ok_or("scope_denied")?;
                let (subject, subject_truncated) = subject(&m.subject);
                let thread = threads.get(&m.id).map(|id| id.0);
                let direction = direction(m, folder);
                // Equal RFC IDs alone are insufficient to deduplicate contradictory evidence.
                if !m.message_id.is_empty()
                    && m.message_id.len() <= mailcraft_model::thread_index::MAX_LINK_ID
                    && !seen.insert((&m.message_id, m.folder, timestamp(m), thread, direction, &m.subject))
                {
                    continue;
                }
                rows.push(Row {
                    id: m.id,
                    message_identity: digest::digest(&digest::SHA256, m.message_id.as_bytes()).as_ref().to_vec(),
                    folder: m.folder,
                    thread,
                    timestamp: timestamp(m),
                    direction,
                    subject,
                    subject_truncated,
                });
            }
            let encoded = serde_json::to_vec(&(identity, account, owner.kind, &kinds, cutoff, &subset, &rows)).map_err(|_| "snapshot_error")?;
            let binding = digest::digest(&digest::SHA256, &encoded).as_ref().to_vec();
            let has = |kind| subset.iter().any(|id| kinds.get(id) == Some(&kind));
            // A remote cache alone does not prove a complete folder on the upstream provider.
            let local = owner.kind == AccountKind::Local;
            let snapshot = Snapshot { rows, binding, inbox: local && has(FolderKind::Inbox), sent: local && has(FolderKind::Sent) };
            views.insert(subset, snapshot);
        }
        Ok(Self { account, cutoff, key: hmac::Key::new(hmac::HMAC_SHA256, secret), views })
    }

    fn token(&self, view: &Snapshot, subject: bool, purpose: u8, value: Id) -> String {
        let mut mac = hmac::Context::with_key(&self.key);
        mac.update(b"mailcraft-scoped-headers-v1");
        mac.update(&view.binding);
        mac.update(&[u8::from(subject), purpose]);
        mac.update(&value.to_be_bytes());
        mac.sign().as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn row(&self, view: &Snapshot, subject: bool, r: &Row) -> Value {
        let mut v = json!({
            "sourceRef": self.token(view, subject, b's', r.id),
            "threadRef": r.thread.map(|t| self.token(view, subject, b't', t)),
            "timestamp": r.timestamp,
            "direction": r.direction
        });
        if subject {
            v["subject"] = json!(r.subject);
            v["subjectTruncated"] = json!(r.subject_truncated);
        }
        v
    }

    pub fn query(&self, name: &str, args: Value) -> Result<Value, &'static str> {
        let mut q: Query = serde_json::from_value(args).map_err(|_| "invalid_arguments")?;
        if q.account != self.account || q.cutoff != self.cutoff || !(1..=2).contains(&q.folders.len()) {
            return Err("scope_denied");
        }
        q.folders.sort_unstable();
        let view = self.views.get(&q.folders).ok_or("scope_denied")?;
        let show_subject = match q.fields.as_slice() {
            [] => false,
            [f] if f == "subject" => true,
            _ => return Err("unsupported_projection"),
        };
        let coverage = json!({"inbox":view.inbox,"sent":view.sent});
        let valid = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        match name {
            "page_message_headers" if q.source_ref.is_none() => {
                let limit = q.limit.unwrap_or(MAX_PAGE);
                if !(1..=MAX_PAGE).contains(&limit) {
                    return Err("invalid_limit");
                }
                let start = match q.cursor {
                    None => 0,
                    Some(token) if valid(&token) => {
                        view.rows.iter().position(|r| self.token(view, show_subject, b'c', r.id) == token).map(|i| i + 1).ok_or("stale_cursor")?
                    }
                    Some(_) => return Err("invalid_cursor"),
                };
                let page = view.rows.iter().skip(start).take(limit);
                let next = if start + limit < view.rows.len() {
                    view.rows.get(start + limit - 1).map(|r| self.token(view, show_subject, b'c', r.id))
                } else {
                    None
                };
                Ok(json!({"rows":page.map(|r| self.row(view, show_subject, r)).collect::<Vec<_>>(),
                    "nextCursor":next,"coverage":coverage,"cutoff":self.cutoff}))
            }
            "resolve_message_evidence" if q.cursor.is_none() && q.limit.is_none() => {
                let token = q.source_ref.filter(|s| valid(s)).ok_or("invalid_reference")?;
                let row = view.rows.iter().find(|r| self.token(view, show_subject, b's', r.id) == token).ok_or("invalid_reference")?;
                Ok(json!({"row":self.row(view, show_subject, row),"coverage":coverage,"cutoff":self.cutoff}))
            }
            _ => Err("invalid_arguments"),
        }
    }
}

/// Open a snapshot through the read-only store entry point. The normal GUI opening path can
/// migrate saves and restore messages, neither of which is part of an agent's read grant.
pub fn open(path: &std::path::Path, scope: &str, cutoff: i64, key_file: Option<&std::path::Path>) -> Result<HeaderReader, String> {
    let (owner, selected) = scope.split_once(':').ok_or("invalid_grant")?;
    let account = owner.parse::<Id>().map_err(|_| "invalid_grant")?;
    let folders = selected.split(',').map(str::parse::<Id>).collect::<Result<Vec<_>, _>>().map_err(|_| "invalid_grant")?;
    let secret = match key_file {
        Some(f) => private_key(f)?,
        None => {
            use ring::rand::SecureRandom;
            let mut bytes = [0; 32];
            ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| "key_unavailable")?;
            bytes
        }
    };
    let canonical = path.canonicalize().map_err(|_| "snapshot_unavailable")?;
    let identity = canonical.to_str().ok_or("unsupported_snapshot_path")?;
    let opened = crate::store::open(&canonical)?;
    if opened.notice().is_some() {
        return Err("incomplete_snapshot".into());
    }
    HeaderReader::new(&opened.mailbox, account, folders, cutoff, identity, &secret).map_err(String::from)
}

fn private_key(path: &std::path::Path) -> Result<[u8; 32], &'static str> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return Err("invalid_key_file");
    }
    let file = options.open(path).map_err(|_| "invalid_key_file")?;
    let meta = file.metadata().map_err(|_| "invalid_key_file")?;
    if !meta.is_file() || meta.len() != 32 {
        return Err("invalid_key_file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("key_file_permissions");
        }
    }
    let mut bytes = Vec::new();
    file.take(33).read_to_end(&mut bytes).map_err(|_| "invalid_key_file")?;
    bytes.try_into().map_err(|_| "invalid_key_file")
}
