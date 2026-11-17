// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The acceptance benchmark of optional vectors (SCN-VECTOR-001): hybrid retrieval over 200,000
//! synthetic messages (the word index and the vectors in memory, as the desktop holds them) must
//! answer within 150 ms at the 95th percentile and stay within 350 MB. Ignored by default (it
//! builds a large mailbox); the boundary runs it in release:
//!
//! ```text
//! /usr/bin/time -l cargo test --release -p mailcraft-engine --lib \
//!     retrieval::vector_bench_tests -- --ignored --nocapture --test-threads 1
//! ```
//!
//! `MAILCRAFT_BENCH_MESSAGES` (200000), `MAILCRAFT_BENCH_DIMS` (384) and
//! `MAILCRAFT_BENCH_QUERIES` (100) change its size. It prints a `vector-stage …` line with this
//! process's resident memory after each stage (the mailbox loaded, the word index built, the
//! vectors loaded, the queries run), then one line, `vector-bench …`, with the numbers, and only
//! once everything is printed does it fail when a threshold is missed. Synthetic text from a
//! fixed seed, invented people at `example.org`.

use std::time::{Duration, Instant};

use mailcraft_model::ai::RetrievalRequest;
use mailcraft_model::{Address, Body, FolderKind, Mailbox, Message};

use super::vectors::{CHUNKS_A_MESSAGE, VectorIndex};
use super::*;
use crate::Session;
use crate::words::{Doc, Index};

/// Different words of the synthetic mail: a vocabulary of about the size of a real mailbox's
/// common words.
const VOCABULARY: u64 = 20_000;

/// A fixed-seed generator (xorshift64*).
struct Seed(u64);

impl Seed {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn word(&mut self) -> String {
        format!("w{}", self.next() % VOCABULARY)
    }

    fn unit(&mut self, dims: usize) -> Vec<f32> {
        (0..dims).map(|_| (self.next() % 2_001) as f32 / 1_000.0 - 1.0).collect()
    }
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// This process's resident memory in bytes, as `ps` says it (`None` where there is no `ps`).
fn resident_bytes() -> Option<u64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse::<u64>().ok().map(|kb| kb * 1024)
}

#[test]
#[ignore = "builds 200,000 messages: run by the boundary, in release"]
fn hybrid_retrieval_at_200k_messages_meets_its_thresholds() {
    let count = env("MAILCRAFT_BENCH_MESSAGES", 200_000);
    let dims = env("MAILCRAFT_BENCH_DIMS", 384);
    let queries = env("MAILCRAFT_BENCH_QUERIES", 100).max(1);
    let mut seed = Seed(20_261_116);

    let mut mb = Mailbox::with_local_account("Robin Owner", "robin@example.org");
    let inbox = mb.folder_of_kind(mb.accounts[0].id, FolderKind::Inbox).unwrap();
    for i in 0..count {
        let subject = format!("{} {} {}", seed.word(), seed.word(), seed.word());
        let body: Vec<String> = (0..40).map(|_| seed.word()).collect();
        let from = Address::new(format!("Person {}", i % 500), format!("p{}@example.org", i % 500));
        mb.add_message(Message {
            folder: inbox,
            from,
            subject,
            date: i as i64,
            body: Body { text: body.join(" "), html: None },
            ..Default::default()
        })
        .unwrap();
    }
    let s = Session::new(mb);
    let shown = |n: Option<u64>| n.map_or("unknown".to_string(), |r| r.to_string());
    let grown = |now: Option<u64>, from: Option<u64>| now.zip(from).map(|(a, b)| a.saturating_sub(b));
    // What the retrieval holds is measured from here: the mailbox itself is held anyway.
    let before = resident_bytes();
    println!("vector-stage stage=mailbox messages={count} rss_bytes={}", shown(before));
    let mut index = Index::new(None);
    for m in s.mailbox.messages.values() {
        index.insert(Doc::of(m, &m.body.text), None);
    }
    let after_words = resident_bytes();
    println!("vector-stage stage=words rss_bytes={} stage_added_bytes={}", shown(after_words), shown(grown(after_words, before)));
    let mut vectors = VectorIndex::new(None);
    vectors.reset("bench-embedder");
    // A bulk load of a counted size: reserved exactly, and nothing left over after it.
    vectors.reserve_exact(s.mailbox.messages.len().saturating_mul(CHUNKS_A_MESSAGE));
    for m in s.mailbox.messages.values() {
        for chunk in 0..CHUNKS_A_MESSAGE as u16 {
            vectors.insert(m.id, 1, chunk, &seed.unit(dims)).unwrap();
        }
    }
    vectors.shrink_to_fit();
    let stored = vectors.storage();
    let after_vectors = resident_bytes();
    println!(
        "vector-stage stage=vectors rss_bytes={} stage_added_bytes={} vector_bytes={} codes_len={} codes_capacity={} metas_len={} metas_capacity={} messages={}",
        shown(after_vectors),
        shown(grown(after_vectors, after_words)),
        vectors.memory_bytes(),
        stored.codes_len,
        stored.codes_capacity,
        stored.metas_len,
        stored.metas_capacity,
        stored.messages,
    );
    // No person, conversation or folder is asked for: no links are needed.
    let graph = MailGraph::default();
    let texts = SessionTexts { session: &s };

    let mut times: Vec<Duration> = Vec::with_capacity(queries);
    let mut over_ten = 0usize;
    for _ in 0..queries {
        let question = format!("What {} did they send about the {}?", seed.word(), seed.word());
        let request = RetrievalRequest::new(&question, Some(10), None, None, None).unwrap();
        let query = seed.unit(dims);
        let started = Instant::now();
        let lexical = LexicalRetriever::new(&index, &graph, &texts);
        let hits = HybridRetriever { lexical, vectors: &vectors, texts: &texts, query: &query }.retrieve(&request).unwrap();
        times.push(started.elapsed());
        // Judged once everything is printed.
        over_ten = over_ten.saturating_add(usize::from(hits.hits.len() > 10));
    }
    times.sort_unstable();
    let p95 = times[(times.len() * 95).div_ceil(100).saturating_sub(1).min(times.len() - 1)];
    let p50 = times[times.len() / 2];
    let rss = resident_bytes();
    let added = rss.zip(before).map(|(after, before)| after.saturating_sub(before));
    println!("vector-stage stage=queries rss_bytes={} stage_added_bytes={}", shown(rss), shown(grown(rss, after_vectors)));
    println!(
        "vector-bench messages={count} dims={dims} passages={} vector_bytes={} p50_ms={} p95_ms={} rss_bytes={} retrieval_rss_bytes={}",
        vectors.len(),
        vectors.memory_bytes(),
        p50.as_millis(),
        p95.as_millis(),
        shown(rss),
        shown(added),
    );
    // Everything is printed: now judged.
    assert_eq!(over_ten, 0, "{over_ten} answers had more than 10 hits");
    assert_eq!((stored.codes_capacity, stored.metas_capacity), (stored.codes_len, stored.metas_len), "the load left room it doesn't use");
    assert!(p95 <= Duration::from_millis(150), "p95 {p95:?} is over 150 ms");
    // The word index and the vectors together, beside the mailbox they are of.
    if let Some(added) = added {
        assert!(added <= 350 << 20, "retrieval holds {added} bytes, over 350 MB");
    }
}
