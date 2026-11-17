// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The vectors of the mailbox's passages, kept beside the mailbox file so that a question finds
//! messages by meaning as well as by words (optional: only once the owner set up an embeddings
//! endpoint, and the host ran the embedding jobs, `ai.embedRequest` / `ai.embedApply`).
//!
//! ```text
//! mailbox.mailcraft           the mailbox (store.rs)
//! mailbox.mailcraft.words     its word index (words.rs)
//! mailbox.mailcraft.vectors   this: for the first passages of each message, a vector
//! ```
//!
//! - **Exact search over int8.** Each vector is made unit length and kept as one byte a
//!   dimension with its scale ([`quantise`]); a question's nearest passages are found by
//!   comparing it with every one ([`VectorIndex::nearest`]). No approximate index.
//! - **Derived data, as the word index.** Everything in it can be had again from the mailbox and
//!   the endpoint, so it is never repaired: a file that is damaged, too large, of another version
//!   or of another embedder (the model's name and the number of dimensions are in its header) is
//!   thrown away and built again. Each message is known by its id and its fingerprint
//!   ([`crate::words::fingerprint`]): one whose fingerprint changed is embedded again.
//! - **The file** is a header and a log of records, each with its length, its kind and a checksum,
//!   appended as vectors come ([`VectorIndex::flush`]); written again whole, to a temporary file
//!   renamed over, when more was taken out than is left ([`VectorIndex::compact`]). A record cut
//!   short at the end is left out with what follows; any other damage makes the whole file go.
//! - **Private as the mail:** the file is for its owner only, and nothing of it is logged.
//! - **Bounded:** at most [`MAX_DIMS`] dimensions, [`MAX_CODE_BYTES`] of codes in memory (past
//!   it no more is taken), [`MAX_FILE`] read from disk; everything read back is checked before
//!   it is believed.
//! - **Reserved exactly:** where the number of passages is known before they come (a file read
//!   back, a compaction, a job of embedding whose size was counted: [`VectorIndex::reserve_exact`])
//!   the codes and the places are reserved for that many and no more, and let go of down to what
//!   is held once the load is over ([`VectorIndex::shrink_to_fit`]), so no growth slack is kept.
//!   Each message's passages are a fixed array of [`CHUNKS_A_MESSAGE`] places, not a list.
//!
//! Not in the browser, which keeps no derived files.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use mailcraft_model::Id;
use mailcraft_model::ai::AiError;

use super::vector::{VectorHit, VectorStore};

/// The format of the file: another number is another version, thrown away and built again.
pub const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"MCVECTS\0";
/// Most dimensions of a vector.
pub const MAX_DIMS: usize = 4_096;
/// Most bytes of codes held (about 2 passages a message of 200,000 messages at 384 dimensions,
/// with room): what the store holds in memory stays far under 350 MB.
pub const MAX_CODE_BYTES: usize = 192 << 20;
/// Most passages held.
pub const MAX_ENTRIES: usize = 2_000_000;
/// Longest embedder name, in bytes.
pub const MAX_EMBEDDER: usize = 200;
/// Largest file read.
pub const MAX_FILE: u64 = 512 << 20;
/// Passages of a message that get a vector (its first ones).
pub const CHUNKS_A_MESSAGE: usize = 2;
/// Most passages asked for at once.
pub const MAX_ASKED: usize = 1_000;
/// A job of embedding is reserved for ([`VectorIndex::plan`]) only when it is at least this many
/// passages more than there is room for: a few more grow the store as they come, so that new
/// mail arriving one message at a time doesn't move the whole store each time.
pub const PLAN_AT_LEAST: usize = 4_096;

/// Where the vectors of the mailbox file at `mailbox` are kept: `<file>.vectors` beside it.
pub fn vectors_file(mailbox: &Path) -> PathBuf {
    crate::store::words_file(mailbox).with_extension("vectors")
}

/// Remove the vectors kept beside the mailbox file at `mailbox` (with the mailbox's other derived
/// files): they are built again when wanted.
pub fn remove(mailbox: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(vectors_file(mailbox)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

const ADD: u8 = 1;
const KILL: u8 = 2;
/// A record's frame: its length, its kind, and after the payload an 8-byte checksum.
const FRAME: u64 = 4 + 1 + 8;
/// An ADD record's payload before its codes: message, fingerprint, passage, scale.
const ADD_HEAD: usize = 8 + 8 + 2 + 4;

/// FNV-1a, 64 bits: the records' checksum.
fn checksum(kind: u8, payload: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in std::iter::once(&kind).chain(payload) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `vector` as it is kept: made unit length, then one byte a dimension and the scale that brings
/// them back. Refused for a vector that has no length or a number that isn't finite.
pub fn quantise(vector: &[f32]) -> Result<(f32, Vec<i8>), AiError> {
    if vector.iter().any(|x| !x.is_finite()) {
        return Err(AiError::Invalid("a vector holds a number that isn't finite".into()));
    }
    let norm = vector.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    if !(norm.is_finite() && norm > 0.0) {
        return Err(AiError::Invalid("a vector has no length".into()));
    }
    let largest = vector.iter().map(|x| (f64::from(*x) / norm).abs()).fold(0.0f64, f64::max);
    if !(largest.is_finite() && largest > 0.0) {
        return Err(AiError::Invalid("a vector has no length".into()));
    }
    let scale = largest / 127.0;
    let codes = vector.iter().map(|x| ((f64::from(*x) / norm) / scale).round().clamp(-127.0, 127.0) as i8).collect();
    Ok((scale as f32, codes))
}

/// `query` made unit length; `None` when it can't be.
fn unit(query: &[f32]) -> Option<Vec<f32>> {
    if query.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let norm = query.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    (norm.is_finite() && norm > 0.0).then(|| query.iter().map(|x| (f64::from(*x) / norm) as f32).collect())
}

/// No passage in this place of a message.
const NO_PLACE: u32 = u32::MAX;

/// A message's fingerprint and where its passages are in the store, by passage: a fixed array,
/// so that a message held costs no allocation of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Places {
    fp: u64,
    at: [u32; CHUNKS_A_MESSAGE],
}

impl Places {
    fn new(fp: u64) -> Places {
        Places { fp, at: [NO_PLACE; CHUNKS_A_MESSAGE] }
    }

    /// The places held, in the store.
    fn held(&self) -> impl Iterator<Item = u32> + '_ {
        self.at.iter().copied().filter(|at| *at != NO_PLACE)
    }
}

/// How much of its storage the store uses: what it holds and what it has room for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Storage {
    pub codes_len: usize,
    pub codes_capacity: usize,
    pub metas_len: usize,
    pub metas_capacity: usize,
    pub messages: usize,
}

/// One passage's place in the store.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Meta {
    message: Id,
    fp: u64,
    chunk: u16,
    scale: f32,
    alive: bool,
}

/// A batch of passages handed to the host to embed, and what it is for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub id: u64,
    pub embedder: String,
    /// Each passage: its message, the message's fingerprint when it was read, its place.
    pub items: Vec<(Id, u64, u16)>,
}

/// The vectors of one mailbox (see the module's documentation).
#[derive(Debug, Default)]
pub struct VectorIndex {
    embedder: String,
    dims: usize,
    metas: Vec<Meta>,
    codes: Vec<i8>,
    /// Each message's fingerprint and its passages' places in `metas`.
    by_message: HashMap<Id, Places>,
    /// Passages a load to come was counted at, before the first vector set the dimensions
    /// ([`VectorIndex::reserve_exact`]).
    planned: usize,
    dead: usize,
    /// The file it is kept in; `None`: memory only.
    file: Option<PathBuf>,
    /// How long the file is as far as it was written; 0: to be written again whole.
    file_len: u64,
    /// Records waiting to be appended.
    pending: Vec<u8>,
    /// Writing failed: it goes on in memory only.
    broken: bool,
    /// The batch handed out last, and the number of the next.
    batch: Option<Batch>,
    batches: u64,
}

impl VectorIndex {
    /// An empty store, kept in `file` when there is one.
    pub fn new(file: Option<PathBuf>) -> VectorIndex {
        VectorIndex { file, ..Default::default() }
    }

    /// The file it is kept in.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// The store kept in `file`: as it was written, or empty (to be written again whole) when it
    /// isn't there or can't be believed.
    pub fn open(file: &Path) -> VectorIndex {
        let mut index = VectorIndex::new(Some(file.to_path_buf()));
        match index.read(file) {
            Ok(len) => index.file_len = len,
            Err(why) => {
                if file.exists() {
                    log::info!("the vectors beside the mailbox are built again: {why}");
                }
                index = VectorIndex::new(Some(file.to_path_buf()));
            }
        }
        index
    }

    pub fn embedder(&self) -> &str {
        &self.embedder
    }

    /// Dimensions of its vectors; 0 before the first.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Passages held.
    pub fn len(&self) -> usize {
        self.metas.len().saturating_sub(self.dead)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes it holds in memory, about (what is reserved, touched or not).
    pub fn memory_bytes(&self) -> usize {
        let meta = std::mem::size_of::<Meta>();
        // A table's slot: the key, the value and a control byte.
        let slot = std::mem::size_of::<Id>().saturating_add(std::mem::size_of::<Places>()).saturating_add(1);
        self.codes
            .capacity()
            .saturating_add(self.metas.capacity().saturating_mul(meta))
            .saturating_add(self.by_message.capacity().saturating_mul(slot))
    }

    /// What it holds and what it has room for.
    pub fn storage(&self) -> Storage {
        Storage {
            codes_len: self.codes.len(),
            codes_capacity: self.codes.capacity(),
            metas_len: self.metas.len(),
            metas_capacity: self.metas.capacity(),
            messages: self.by_message.len(),
        }
    }

    /// Room for `passages` more, and no more than that: the places, and the codes once the
    /// dimensions are known (else when the first vector sets them). Within the caps
    /// ([`MAX_ENTRIES`], [`MAX_CODE_BYTES`]); nothing is reserved that is already there.
    pub fn reserve_exact(&mut self, passages: usize) {
        let passages = passages.min(MAX_ENTRIES.saturating_sub(self.metas.len()));
        self.metas.reserve_exact(passages);
        self.by_message.reserve(passages.div_ceil(CHUNKS_A_MESSAGE));
        if self.dims == 0 {
            self.planned = passages;
            return;
        }
        let bytes = passages.saturating_mul(self.dims).min(MAX_CODE_BYTES.saturating_sub(self.codes.len()));
        self.codes.reserve_exact(bytes);
        self.planned = 0;
    }

    /// Plan for `passages` more to come (a job of embedding, counted): reserved exactly
    /// ([`VectorIndex::reserve_exact`]) when that is [`PLAN_AT_LEAST`] or more beyond the room
    /// there is.
    pub fn plan(&mut self, passages: usize) {
        let spare = self.metas.capacity().saturating_sub(self.metas.len());
        if passages >= spare.saturating_add(PLAN_AT_LEAST) {
            self.reserve_exact(passages);
        }
    }

    /// Let go of the room that isn't used: after a load is over (a file read back, a compaction,
    /// a job of embedding that has nothing left to do).
    pub fn shrink_to_fit(&mut self) {
        self.codes.shrink_to_fit();
        self.metas.shrink_to_fit();
        self.by_message.shrink_to_fit();
        self.pending.shrink_to_fit();
        self.planned = 0;
    }

    /// Whether more vectors fit.
    pub fn full(&self) -> bool {
        self.metas.len() >= MAX_ENTRIES || (self.dims > 0 && self.codes.len().saturating_add(self.dims) > MAX_CODE_BYTES)
    }

    /// Whether message `id` is held as `fp` says it is now.
    pub fn current(&self, id: Id, fp: u64) -> bool {
        self.by_message.get(&id).is_some_and(|p| p.fp == fp)
    }

    /// The messages it holds, with their fingerprints.
    pub fn messages(&self) -> impl Iterator<Item = (Id, u64)> + '_ {
        self.by_message.iter().map(|(id, p)| (*id, p.fp))
    }

    /// Start again for `embedder`: everything held goes (the file is written again whole).
    pub fn reset(&mut self, embedder: &str) {
        let file = self.file.take();
        let batches = self.batches;
        *self = VectorIndex::new(file);
        self.batches = batches;
        self.embedder = mailcraft_model::ai::clean_text(embedder, MAX_EMBEDDER).0;
    }

    /// Hand out a batch of passages to embed (it takes the place of the one before).
    pub fn hand_out(&mut self, items: Vec<(Id, u64, u16)>) -> &Batch {
        self.batches = self.batches.wrapping_add(1).max(1);
        self.batch.insert(Batch { id: self.batches, embedder: self.embedder.clone(), items })
    }

    /// The batch handed out last, if it is `id`.
    pub fn waiting(&self, id: u64) -> Option<&Batch> {
        self.batch.as_ref().filter(|b| b.id == id)
    }

    /// The batch handed out last, if it is `id`; it is taken.
    pub fn take_batch(&mut self, id: u64) -> Option<Batch> {
        if self.batch.as_ref().is_some_and(|b| b.id == id) { self.batch.take() } else { None }
    }

    /// Take message `id` out.
    pub fn remove(&mut self, id: Id) {
        let Some(places) = self.by_message.remove(&id) else { return };
        for at in places.held() {
            self.kill(at);
        }
        record(&mut self.pending, KILL, &id.to_le_bytes());
    }

    /// The passage at `at` taken out (counted once).
    fn kill(&mut self, at: u32) {
        if let Some(m) = self.metas.get_mut(at as usize)
            && m.alive
        {
            m.alive = false;
            self.dead = self.dead.saturating_add(1);
        }
    }

    /// Keep `vector` for passage `chunk` of message `id` (whose fingerprint is `fp`). The first
    /// vector sets the dimensions; one of others is refused, and so is one past the caps or of a
    /// passage past the first [`CHUNKS_A_MESSAGE`]. A passage kept again takes the place of the
    /// one before.
    pub fn insert(&mut self, id: Id, fp: u64, chunk: u16, vector: &[f32]) -> Result<(), AiError> {
        if vector.is_empty() || vector.len() > MAX_DIMS {
            return Err(AiError::Invalid(format!("a vector has 1 to {MAX_DIMS} numbers")));
        }
        if usize::from(chunk) >= CHUNKS_A_MESSAGE {
            return Err(AiError::Invalid(format!("only the first {CHUNKS_A_MESSAGE} passages of a message get a vector")));
        }
        if self.dims == 0 && self.is_empty() {
            self.dims = vector.len();
            if self.planned > 0 {
                let planned = self.planned;
                self.reserve_exact(planned);
            }
        }
        if vector.len() != self.dims {
            return Err(AiError::Invalid(format!("a vector has {} numbers, not {}", vector.len(), self.dims)));
        }
        if self.full() {
            return Err(AiError::TooMany { what: "bytes of vectors", max: MAX_CODE_BYTES });
        }
        let (scale, codes) = quantise(vector)?;
        let mut payload = Vec::with_capacity(ADD_HEAD.saturating_add(codes.len()));
        payload.extend_from_slice(&id.to_le_bytes());
        payload.extend_from_slice(&fp.to_le_bytes());
        payload.extend_from_slice(&chunk.to_le_bytes());
        payload.extend_from_slice(&scale.to_bits().to_le_bytes());
        payload.extend(codes.iter().map(|c| c.to_le_bytes()[0]));
        record(&mut self.pending, ADD, &payload);
        self.add(Meta { message: id, fp, chunk, scale, alive: true }, &codes);
        Ok(())
    }

    /// Keep `meta` and its `codes` (of a passage below [`CHUNKS_A_MESSAGE`]: refused otherwise).
    fn add(&mut self, meta: Meta, codes: &[i8]) {
        let Ok(at) = u32::try_from(self.metas.len()) else { return };
        if at == NO_PLACE {
            return;
        }
        let slot = usize::from(meta.chunk);
        let mut places = self.by_message.get(&meta.message).copied().unwrap_or_else(|| Places::new(meta.fp));
        let Some(old) = places.at.get(slot).copied() else { return };
        if places.fp != meta.fp {
            // Another reading of the message: what was held of the one before goes.
            for gone in places.held() {
                self.kill(gone);
            }
            places = Places::new(meta.fp);
        } else if old != NO_PLACE {
            // The same passage again: the one before goes.
            self.kill(old);
        }
        if let Some(s) = places.at.get_mut(slot) {
            *s = at;
        }
        self.by_message.insert(meta.message, places);
        self.metas.push(meta);
        self.codes.extend_from_slice(codes);
    }

    /// Whether so much was taken out that it is worth writing again whole.
    pub fn wants_compacting(&self) -> bool {
        self.dead > 4_096 && self.dead > self.len()
    }

    /// Let go of what was taken out, and have the file written again whole at the next flush.
    pub fn compact(&mut self) {
        let dims = self.dims;
        let live = self.len();
        let metas = std::mem::take(&mut self.metas);
        let codes = std::mem::take(&mut self.codes);
        self.by_message.clear();
        self.dead = 0;
        self.reserve_exact(live);
        for (i, m) in metas.into_iter().enumerate() {
            if !m.alive {
                continue;
            }
            let start = i.saturating_mul(dims);
            if let Some(c) = codes.get(start..start.saturating_add(dims)) {
                self.add(m, c);
            }
        }
        self.pending.clear();
        self.file_len = 0;
        self.shrink_to_fit();
    }

    /// Write what changed to the file. A file that can't be written leaves the store in memory
    /// only (it is built again another time).
    pub fn flush(&mut self) {
        let Some(file) = self.file.clone().filter(|_| !self.broken) else {
            self.pending.clear();
            return;
        };
        if self.file_len > 0 && self.pending.is_empty() {
            return;
        }
        let written = if self.file_len == 0 { self.write_whole(&file) } else { self.append(&file) };
        match written {
            Ok(len) => {
                self.file_len = len;
                self.pending.clear();
            }
            Err(e) => {
                log::warn!("the vectors beside the mailbox couldn't be written ({}); they are kept in memory only", e.kind());
                self.broken = true;
                self.pending.clear();
            }
        }
    }

    fn header(&self) -> Vec<u8> {
        let name = mailcraft_model::ai::clean_text(&self.embedder, MAX_EMBEDDER).0;
        let mut h = Vec::with_capacity(32usize.saturating_add(name.len()));
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&VERSION.to_le_bytes());
        h.extend_from_slice(&u32::try_from(self.dims).unwrap_or(0).to_le_bytes());
        h.extend_from_slice(&u16::try_from(name.len()).unwrap_or(0).to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        let sum = checksum(0, h.get(8..).unwrap_or(&[]));
        h.extend_from_slice(&sum.to_le_bytes());
        h
    }

    /// The file written again whole, to a temporary file renamed over it.
    fn write_whole(&self, file: &Path) -> std::io::Result<u64> {
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut f = std::io::BufWriter::new(crate::store::create_private_replacing(&tmp)?);
        let mut len = 0u64;
        let header = self.header();
        f.write_all(&header)?;
        len = len.saturating_add(header.len() as u64);
        let mut out = Vec::new();
        for (i, m) in self.metas.iter().enumerate() {
            if !m.alive {
                continue;
            }
            let start = i.saturating_mul(self.dims);
            let Some(codes) = self.codes.get(start..start.saturating_add(self.dims)) else { continue };
            let mut payload = Vec::with_capacity(ADD_HEAD.saturating_add(codes.len()));
            payload.extend_from_slice(&m.message.to_le_bytes());
            payload.extend_from_slice(&m.fp.to_le_bytes());
            payload.extend_from_slice(&m.chunk.to_le_bytes());
            payload.extend_from_slice(&m.scale.to_bits().to_le_bytes());
            payload.extend(codes.iter().map(|c| c.to_le_bytes()[0]));
            out.clear();
            record(&mut out, ADD, &payload);
            f.write_all(&out)?;
            len = len.saturating_add(out.len() as u64);
        }
        let f = f.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, file)?;
        Ok(len)
    }

    /// The records waiting appended after the file's first `file_len` bytes.
    fn append(&self, file: &Path) -> std::io::Result<u64> {
        let mut f = std::fs::OpenOptions::new().write(true).open(file)?;
        f.set_len(self.file_len)?;
        std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(self.file_len))?;
        f.write_all(&self.pending)?;
        Ok(self.file_len.saturating_add(self.pending.len() as u64))
    }

    /// Read `file` into this (empty) store: how long it is as far as it was read whole.
    fn read(&mut self, file: &Path) -> Result<u64, String> {
        let f = std::fs::File::open(file).map_err(|e| format!("it can't be read ({})", e.kind()))?;
        let len = f.metadata().map_err(|e| format!("it can't be read ({})", e.kind()))?.len();
        if len > MAX_FILE {
            return Err("it is larger than a store is ever written".into());
        }
        let mut r = std::io::BufReader::with_capacity(1 << 20, f);
        // The header.
        let mut fixed = [0u8; 18];
        r.read_exact(&mut fixed).map_err(|_| "it is cut short".to_string())?;
        if fixed.get(..8) != Some(MAGIC.as_slice()) {
            return Err("it isn't a store of vectors".into());
        }
        let u32_at = |b: &[u8], at: usize| b.get(at..at.saturating_add(4)).and_then(|v| <[u8; 4]>::try_from(v).ok()).map(u32::from_le_bytes);
        let version = u32_at(&fixed, 8).unwrap_or(0);
        if version != VERSION {
            return Err(format!("it is of another version ({version})"));
        }
        let dims = u32_at(&fixed, 12).map_or(0, |d| d as usize);
        let name_len = fixed.get(16..18).and_then(|v| <[u8; 2]>::try_from(v).ok()).map_or(0, |v| usize::from(u16::from_le_bytes(v)));
        if dims > MAX_DIMS || name_len > MAX_EMBEDDER {
            return Err("its header can't be believed".into());
        }
        let mut name = vec![0u8; name_len];
        r.read_exact(&mut name).map_err(|_| "it is cut short".to_string())?;
        let mut sum = [0u8; 8];
        r.read_exact(&mut sum).map_err(|_| "it is cut short".to_string())?;
        let mut checked = fixed.get(8..).unwrap_or(&[]).to_vec();
        checked.extend_from_slice(&name);
        if checksum(0, &checked) != u64::from_le_bytes(sum) {
            return Err("its header is damaged".into());
        }
        self.embedder = String::from_utf8(name).map_err(|_| "its header is damaged".to_string())?;
        self.dims = dims;
        let header_len = (18usize.saturating_add(name_len).saturating_add(8)) as u64;
        // Room for as many passages as the file can hold, at most (exactly as many when nothing
        // was taken out), let go of down to what is held once it is read.
        if self.dims > 0 {
            let add = FRAME.saturating_add(ADD_HEAD.saturating_add(self.dims) as u64);
            let most = len.saturating_sub(header_len) / add.max(1);
            self.reserve_exact(usize::try_from(most).unwrap_or(MAX_ENTRIES));
        }
        let mut at = header_len;
        let mut frame = [0u8; 5];
        let mut payload = Vec::new();
        // At most one step a record, each at least FRAME bytes of a bounded file.
        while len.saturating_sub(at) >= FRAME {
            r.read_exact(&mut frame).map_err(|_| "it can't be read".to_string())?;
            let size = u64::from(u32_at(&frame, 0).unwrap_or(u32::MAX));
            let kind = frame.get(4).copied().unwrap_or(0);
            if len.saturating_sub(at).saturating_sub(FRAME) < size {
                // The last record was cut short: the program stopped while writing it.
                break;
            }
            let expected = match kind {
                ADD => ADD_HEAD.saturating_add(self.dims) as u64,
                KILL => 8,
                _ => return Err("a record is of no known kind".into()),
            };
            if size != expected || self.dims == 0 && kind == ADD {
                return Err("a record isn't as long as its kind".into());
            }
            payload.clear();
            payload.resize(size as usize, 0);
            r.read_exact(&mut payload).map_err(|_| "it can't be read".to_string())?;
            r.read_exact(&mut sum).map_err(|_| "it can't be read".to_string())?;
            if checksum(kind, &payload) != u64::from_le_bytes(sum) {
                return Err("a record is damaged".into());
            }
            self.take(kind, &payload)?;
            at = at.saturating_add(FRAME).saturating_add(size);
        }
        self.shrink_to_fit();
        Ok(at)
    }

    /// A record read back, believed only once checked.
    fn take(&mut self, kind: u8, payload: &[u8]) -> Result<(), String> {
        let u64_at = |at: usize| payload.get(at..at.saturating_add(8)).and_then(|v| <[u8; 8]>::try_from(v).ok()).map(u64::from_le_bytes);
        let message = u64_at(0).ok_or("a record is cut short")?;
        if message == 0 {
            return Err("a record names no message".into());
        }
        if kind == KILL {
            let pending = std::mem::take(&mut self.pending);
            self.remove(message);
            self.pending = pending;
            return Ok(());
        }
        let fp = u64_at(8).ok_or("a record is cut short")?;
        let chunk = payload.get(16..18).and_then(|v| <[u8; 2]>::try_from(v).ok()).map(u16::from_le_bytes).ok_or("a record is cut short")?;
        let scale = payload
            .get(18..22)
            .and_then(|v| <[u8; 4]>::try_from(v).ok())
            .map(|b| f32::from_bits(u32::from_le_bytes(b)))
            .ok_or("a record is cut short")?;
        if !(scale.is_finite() && scale > 0.0 && scale <= 1.0) {
            return Err("a record's scale can't be believed".into());
        }
        let codes: Vec<i8> = payload.get(ADD_HEAD..).ok_or("a record is cut short")?.iter().map(|b| i8::from_le_bytes([*b])).collect();
        if codes.len() != self.dims || self.full() {
            return Err("there are more vectors than a store holds".into());
        }
        if usize::from(chunk) >= CHUNKS_A_MESSAGE {
            return Err("a record is of a passage that gets no vector".into());
        }
        self.add(Meta { message, fp, chunk, scale, alive: true }, &codes);
        Ok(())
    }
}

fn record(out: &mut Vec<u8>, kind: u8, payload: &[u8]) {
    out.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out.extend_from_slice(&checksum(kind, payload).to_le_bytes());
}

impl VectorStore for VectorIndex {
    fn dims(&self) -> usize {
        self.dims
    }

    fn nearest(&self, query: &[f32], k: usize) -> Result<Vec<VectorHit>, AiError> {
        if self.dims == 0 || self.is_empty() {
            return Ok(Vec::new());
        }
        if query.len() != self.dims {
            return Err(AiError::Invalid(format!("the question's vector has {} numbers, not {}", query.len(), self.dims)));
        }
        let Some(q) = unit(query) else { return Err(AiError::Invalid("the question's vector has no length".into())) };
        let k = k.clamp(1, MAX_ASKED);
        let mut scored: Vec<(f32, u32)> = Vec::with_capacity(self.len());
        for (i, (m, codes)) in self.metas.iter().zip(self.codes.chunks_exact(self.dims)).enumerate() {
            if !m.alive {
                continue;
            }
            let dot: f32 = codes.iter().zip(&q).map(|(c, x)| f32::from(*c) * x).sum();
            let similarity = dot * m.scale;
            if similarity.is_finite()
                && let Ok(i) = u32::try_from(i)
            {
                scored.push((similarity, i));
            }
        }
        if scored.len() > k {
            scored.select_nth_unstable_by(k, |a, b| b.0.total_cmp(&a.0));
            scored.truncate(k);
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        Ok(scored
            .into_iter()
            .filter_map(|(similarity, i)| self.metas.get(i as usize).map(|m| VectorHit { message: m.message, chunk: m.chunk, similarity }))
            .collect())
    }
}
