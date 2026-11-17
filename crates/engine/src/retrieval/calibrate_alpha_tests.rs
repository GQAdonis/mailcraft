// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calibration of the relative cutoff ([`super::lexical::ALPHA`]), run once by hand on a
//! calibration corpus and never on the graded one (DEV-1):
//!
//! ```text
//! MAILCRAFT_CALIBRATION_CORPUS=<dir> cargo test --release -p mailcraft-engine --lib \
//!     retrieval::calibrate_alpha -- --ignored --nocapture
//! ```
//!
//! `<dir>` is what the boundary's corpus builder writes: `corpus.mbox` (RFC 4155) and
//! `manifest.json`, whose `questions` are `{"id", "question", "gold": [Message-ID]}`. A path to
//! the `manifest.json` itself is taken too (the mbox beside it). It sweeps α from 0.30 to 0.90 in
//! steps of 0.05 and prints, for each, the mean recall@10 and precision@10 over the questions as
//! the retrieval grader computes them (`|top10 ∩ gold| / |gold|` and `/ |top10|`, 0 for no hits).
//! It prints only and never asserts: choosing α is a decision recorded with its numbers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mailcraft_model::ai::RetrievalRequest;
use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::Value;

use super::*;
use crate::Session;

/// Where the corpus is named.
const CORPUS_VAR: &str = "MAILCRAFT_CALIBRATION_CORPUS";
/// The sweep: α = 0.30 + 0.05 × step, for step 0 to 12 (0.30 to 0.90).
const STEPS: u32 = 12;
/// The hits graded, as the grader counts them.
const K: usize = 10;

/// A Message-ID as one key, with or without its angle brackets.
fn key(message_id: &str) -> String {
    message_id.trim().trim_start_matches('<').trim_end_matches('>').to_string()
}

/// The mbox and the manifest named by `path` (a directory, or the manifest in one).
fn files(path: &Path) -> (PathBuf, PathBuf) {
    if path.is_dir() {
        return (path.join("corpus.mbox"), path.join("manifest.json"));
    }
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (dir.join("corpus.mbox"), path.to_path_buf())
}

#[test]
#[ignore = "calibration (DEV-1): run by hand with MAILCRAFT_CALIBRATION_CORPUS; prints only"]
fn calibrate_alpha() {
    let Some(path) = std::env::var_os(CORPUS_VAR) else {
        println!("calibrate-alpha: {CORPUS_VAR} is not set; nothing to calibrate");
        return;
    };
    let (mbox, manifest) = files(Path::new(&path));
    let (Ok(bytes), Ok(text)) = (std::fs::read(&mbox), std::fs::read_to_string(&manifest)) else {
        println!("calibrate-alpha: can't read {} and {}", mbox.display(), manifest.display());
        return;
    };
    let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
        println!("calibrate-alpha: {} isn't JSON", manifest.display());
        return;
    };

    let mut mb = Mailbox::with_local_account("Calibration Owner", "owner@example.org");
    let account = mb.accounts.first().map(|a| a.id).unwrap_or_default();
    let Some(inbox) = mb.folder_of_kind(account, FolderKind::Inbox).or_else(|| mb.folders.first().map(|f| f.id)) else {
        println!("calibrate-alpha: the mailbox has no folder");
        return;
    };
    let mut by_message_id: HashMap<String, Id> = HashMap::new();
    for mut m in crate::mime::mbox_messages(&bytes) {
        m.folder = inbox;
        let mid = key(&m.message_id);
        if let Ok(id) = mb.add_message(m) {
            by_message_id.insert(mid, id);
        }
    }
    let s = Session::new(mb);

    // The gold questions, with their gold messages as MailCraft's ids.
    let mut questions: Vec<(String, String, HashSet<Id>)> = Vec::new();
    let mut unmapped = 0usize;
    for q in manifest.get("questions").and_then(Value::as_array).into_iter().flatten() {
        let (Some(id), Some(question)) = (q.get("id").and_then(Value::as_str), q.get("question").and_then(Value::as_str)) else { continue };
        let mut gold = HashSet::new();
        for g in q.get("gold").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            match by_message_id.get(&key(g)) {
                Some(found) => {
                    gold.insert(*found);
                }
                None => unmapped = unmapped.saturating_add(1),
            }
        }
        questions.push((id.to_string(), question.to_string(), gold));
    }
    println!(
        "calibrate-alpha corpus: {} messages, seed {}, {} questions, {} gold ids not found",
        by_message_id.len(),
        manifest.get("seed").map_or_else(|| "?".to_string(), Value::to_string),
        questions.len(),
        unmapped
    );
    if questions.is_empty() {
        println!("calibrate-alpha: the manifest has no questions");
        return;
    }

    let threads = s.threads();
    let graph = MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads);
    let scan = SessionScan::all(&s);
    let texts = SessionTexts { session: &s };
    let mut retriever = LexicalRetriever::new(&scan, &graph, &texts);
    let n = questions.len() as f64;
    for step in 0..=STEPS {
        let alpha = (30.0 + 5.0 * f64::from(step)) / 100.0;
        retriever.alpha = alpha;
        let (mut recall, mut precision, mut kept) = (0.0f64, 0.0f64, 0usize);
        let mut perfect = 0usize;
        for (_, question, gold) in &questions {
            let Ok(request) = RetrievalRequest::new(question, Some(K as u64), None, None, None) else { continue };
            let Ok(found) = retriever.retrieve(&request) else { continue };
            let top: Vec<Id> = found.hits.iter().take(K).map(|h| h.message).collect();
            let hit = top.iter().filter(|id| gold.contains(id)).count() as f64;
            if !gold.is_empty() {
                recall += hit / gold.len() as f64;
            }
            if !top.is_empty() {
                precision += hit / top.len() as f64;
            }
            if !gold.is_empty() && gold.iter().all(|g| top.contains(g)) {
                perfect = perfect.saturating_add(1);
            }
            kept = kept.saturating_add(top.len());
        }
        println!(
            "calibrate-alpha alpha={alpha:.2} recall@10={:.4} precision@10={:.4} mean_hits={:.2} all_gold_kept={perfect}/{}",
            recall / n,
            precision / n,
            kept as f64 / n,
            questions.len()
        );
    }
    // Which questions lose a gold message at the placeholder, to read beside the numbers.
    retriever.alpha = super::lexical::ALPHA;
    for (id, question, gold) in &questions {
        let Ok(request) = RetrievalRequest::new(question, Some(K as u64), None, None, None) else { continue };
        let Ok(found) = retriever.retrieve(&request) else { continue };
        let top: HashSet<Id> = found.hits.iter().take(K).map(|h| h.message).collect();
        let missing = gold.iter().filter(|g| !top.contains(g)).count();
        if missing > 0 {
            println!("calibrate-alpha at alpha={:.2}: {id} misses {missing} of {} gold", super::lexical::ALPHA, gold.len());
        }
    }
}
