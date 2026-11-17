// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::path::PathBuf;

use super::vector::VectorStore;
use super::vectors::*;

/// A folder of its own under the system's temporary folder, removed afterwards.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mailcraft-vectors-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft.vectors")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A vector pointing mostly along axis `axis` of `dims`.
fn along(axis: usize, dims: usize) -> Vec<f32> {
    (0..dims).map(|i| if i == axis { 1.0 } else { 0.05 }).collect()
}

fn filled() -> VectorIndex {
    let mut v = VectorIndex::new(None);
    v.reset("fake-embedder");
    for id in 1..=8u64 {
        v.insert(id, id * 10, 0, &along(id as usize % 8, 8)).unwrap();
    }
    v
}

#[test]
fn quantised_vectors_keep_their_direction() {
    let (scale, codes) = quantise(&[3.0, -4.0]).unwrap();
    // Unit length (0.6, -0.8): the largest is 127.
    assert_eq!(codes, [95, -127]);
    assert!((scale - 0.8 / 127.0).abs() < 1e-6);
    for bad in [vec![0.0, 0.0], vec![f32::NAN, 1.0], vec![f32::INFINITY], vec![]] {
        assert!(quantise(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_nearest_passages_come_first_exactly() {
    let v = filled();
    let hits = v.nearest(&along(3, 8), 3).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].message, 3);
    assert!(hits[0].similarity > 0.9 && hits[0].similarity >= hits[1].similarity);
    // Other lengths are refused; a question without length too.
    assert!(v.nearest(&along(3, 9), 3).is_err());
    assert!(v.nearest(&[0.0; 8], 3).is_err());
    assert!(v.nearest(&[f32::NAN; 8], 3).is_err());
    // Nothing kept: nothing found.
    assert!(VectorIndex::new(None).nearest(&along(1, 8), 3).unwrap().is_empty());
}

#[test]
fn the_first_vector_sets_the_length_and_others_are_refused() {
    let mut v = VectorIndex::new(None);
    v.reset("m");
    v.insert(1, 1, 0, &along(0, 4)).unwrap();
    assert_eq!(v.dims(), 4);
    assert!(v.insert(2, 1, 0, &along(0, 5)).is_err());
    assert!(v.insert(2, 1, 0, &[1.0; MAX_DIMS + 1]).is_err());
    assert_eq!(v.len(), 1);
}

#[test]
fn a_message_read_again_or_taken_out_has_its_vectors_go() {
    let mut v = filled();
    assert!(v.current(3, 30));
    v.insert(3, 31, 0, &along(5, 8)).unwrap();
    assert!(v.current(3, 31) && !v.current(3, 30));
    assert_eq!(v.len(), 8);
    v.remove(3);
    assert_eq!(v.len(), 7);
    assert!(v.nearest(&along(3, 8), 8).unwrap().iter().all(|h| h.message != 3));
    v.compact();
    assert_eq!(v.len(), 7);
    assert!(v.nearest(&along(4, 8), 1).unwrap()[0].message == 4);
}

#[test]
fn the_file_is_read_back_as_it_was_written() {
    let t = Temp::new("round");
    let mut v = filled();
    let mut kept = VectorIndex::new(Some(t.file()));
    kept.reset("fake-embedder");
    for id in 1..=8u64 {
        kept.insert(id, id * 10, 1, &along(id as usize % 8, 8)).unwrap();
    }
    kept.flush();
    kept.remove(2);
    kept.insert(9, 90, 0, &along(2, 8)).unwrap();
    kept.flush();
    let back = VectorIndex::open(&t.file());
    assert_eq!((back.embedder(), back.dims(), back.len()), ("fake-embedder", 8, 8));
    assert!(back.current(9, 90) && !back.current(2, 20));
    assert_eq!(back.nearest(&along(2, 8), 1).unwrap()[0].message, 9);
    // Written again whole after a compaction, it reads the same.
    let mut again = VectorIndex::open(&t.file());
    again.compact();
    again.flush();
    assert_eq!(VectorIndex::open(&t.file()).len(), 8);
    v.flush();
}

#[test]
fn a_record_cut_short_at_the_end_is_left_out_and_damage_rebuilds() {
    let t = Temp::new("damage");
    let mut kept = VectorIndex::new(Some(t.file()));
    kept.reset("m");
    kept.insert(1, 1, 0, &along(0, 8)).unwrap();
    kept.flush();
    kept.insert(2, 2, 0, &along(1, 8)).unwrap();
    kept.flush();
    let bytes = std::fs::read(t.file()).unwrap();
    // Cut short: the first record stays.
    std::fs::write(t.file(), &bytes[..bytes.len() - 5]).unwrap();
    let back = VectorIndex::open(&t.file());
    assert_eq!(back.len(), 1);
    assert!(back.current(1, 1));
    // A byte changed in a record: thrown away, empty.
    let mut damaged = bytes.clone();
    let at = damaged.len() - 20;
    damaged[at] ^= 0xff;
    std::fs::write(t.file(), &damaged).unwrap();
    assert!(VectorIndex::open(&t.file()).is_empty());
    // Not a store at all, another version, a header that lies: empty.
    for junk in [b"not a store of vectors at all".to_vec(), Vec::new(), vec![0xff; 4096]] {
        std::fs::write(t.file(), &junk).unwrap();
        let v = VectorIndex::open(&t.file());
        assert!(v.is_empty() && v.dims() == 0);
    }
    let mut other = bytes.clone();
    other[8] = 9;
    std::fs::write(t.file(), &other).unwrap();
    assert!(VectorIndex::open(&t.file()).is_empty());
    // Missing: empty, and written whole at the next flush.
    std::fs::remove_file(t.file()).unwrap();
    let mut fresh = VectorIndex::open(&t.file());
    fresh.reset("m");
    fresh.insert(5, 5, 0, &along(0, 8)).unwrap();
    fresh.flush();
    assert_eq!(VectorIndex::open(&t.file()).len(), 1);
}

#[test]
fn another_embedder_starts_again() {
    let mut v = filled();
    v.reset("another");
    assert!(v.is_empty() && v.dims() == 0 && v.embedder() == "another");
    v.insert(1, 1, 0, &along(0, 3)).unwrap();
    assert_eq!(v.dims(), 3);
}

#[test]
fn a_batch_is_taken_once_and_only_the_last() {
    let mut v = VectorIndex::new(None);
    v.reset("m");
    let first = v.hand_out(vec![(1, 1, 0)]).id;
    let second = v.hand_out(vec![(2, 2, 0)]).id;
    assert_ne!(first, second);
    assert!(v.take_batch(first).is_none());
    assert_eq!(v.take_batch(second).map(|b| b.items), Some(vec![(2, 2, 0)]));
    assert!(v.take_batch(second).is_none());
}

#[test]
fn a_counted_load_is_reserved_exactly_and_holds_no_slack_after() {
    let mut v = VectorIndex::new(None);
    v.reset("m");
    // Counted before the first vector: the codes are reserved once it sets the dimensions.
    v.reserve_exact(100);
    assert!(v.storage().metas_capacity >= 100);
    for id in 1..=50u64 {
        for chunk in 0..CHUNKS_A_MESSAGE as u16 {
            v.insert(id, 1, chunk, &along(id as usize % 8, 8)).unwrap();
        }
    }
    let used = v.storage();
    assert_eq!((used.metas_len, used.codes_len, used.messages), (100, 800, 50));
    assert_eq!((used.metas_capacity, used.codes_capacity), (100, 800), "no growth past what was counted");
    // Over-counted, then let go of.
    let mut v = VectorIndex::new(None);
    v.reset("m");
    v.reserve_exact(1_000);
    v.insert(1, 1, 0, &along(0, 8)).unwrap();
    assert!(v.storage().codes_capacity >= 8_000);
    v.shrink_to_fit();
    let used = v.storage();
    assert_eq!((used.metas_capacity, used.codes_capacity), (used.metas_len, used.codes_len));
    // Within the caps, whatever is asked.
    let mut v = VectorIndex::new(None);
    v.reset("m");
    v.insert(1, 1, 0, &along(0, 8)).unwrap();
    v.reserve_exact(usize::MAX);
    assert!(v.storage().metas_capacity <= MAX_ENTRIES && v.storage().codes_capacity <= MAX_CODE_BYTES);
}

#[test]
fn a_job_is_planned_for_only_when_it_is_large() {
    let mut v = filled();
    v.shrink_to_fit();
    let before = v.storage().metas_capacity;
    v.plan(10);
    assert_eq!(v.storage().metas_capacity, before, "a few more grow as they come");
    v.plan(PLAN_AT_LEAST * 2);
    assert!(v.storage().metas_capacity >= v.storage().metas_len + PLAN_AT_LEAST * 2);
}

#[test]
fn a_store_read_back_or_compacted_holds_no_slack() {
    let t = Temp::new("slack");
    let mut kept = VectorIndex::new(Some(t.file()));
    kept.reset("m");
    for id in 1..=40u64 {
        kept.insert(id, 1, 0, &along(id as usize % 8, 8)).unwrap();
        kept.insert(id, 1, 1, &along((id as usize + 1) % 8, 8)).unwrap();
    }
    kept.flush();
    for id in 1..=10u64 {
        kept.remove(id);
    }
    kept.flush();
    let back = VectorIndex::open(&t.file());
    assert_eq!(back.len(), 60);
    let read = back.storage();
    assert_eq!((read.metas_capacity, read.codes_capacity), (read.metas_len, read.codes_len), "{read:?}");
    let mut kept = back;
    kept.compact();
    let compacted = kept.storage();
    assert_eq!((compacted.metas_len, compacted.codes_len, compacted.messages), (60, 480, 30));
    assert_eq!((compacted.metas_capacity, compacted.codes_capacity), (60, 480));
}

#[test]
fn each_message_holds_a_fixed_number_of_passages() {
    let mut v = VectorIndex::new(None);
    v.reset("m");
    v.insert(1, 1, 0, &along(0, 8)).unwrap();
    // Past the passages that get a vector: refused, nothing kept.
    assert!(v.insert(1, 1, CHUNKS_A_MESSAGE as u16, &along(1, 8)).is_err());
    assert!(v.insert(1, 1, u16::MAX, &along(1, 8)).is_err());
    assert_eq!(v.len(), 1);
    // The same passage again takes the place of the one before.
    v.insert(1, 1, 0, &along(3, 8)).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v.nearest(&along(3, 8), 1).unwrap()[0].message, 1);
    assert!(v.nearest(&along(3, 8), 1).unwrap()[0].similarity > 0.9);
    // Both passages, then another reading: both go, one new is held.
    v.insert(1, 1, 1, &along(4, 8)).unwrap();
    assert_eq!(v.len(), 2);
    v.insert(1, 2, 1, &along(5, 8)).unwrap();
    assert_eq!(v.len(), 1);
    assert!(v.current(1, 2));
    v.remove(1);
    assert!(v.is_empty());
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]
    #[test]
    fn any_file_is_read_without_panicking(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2_000), keep_header in proptest::prelude::any::<bool>()) {
        let t = Temp::new("fuzz");
        let mut data = bytes;
        if keep_header {
            let mut v = VectorIndex::new(Some(t.file()));
            v.reset("m");
            let _ = v.insert(1, 1, 0, &along(0, 4));
            v.flush();
            let mut head = std::fs::read(t.file()).unwrap_or_default();
            head.truncate(32);
            head.extend_from_slice(&data);
            data = head;
        }
        std::fs::write(t.file(), &data).unwrap();
        let v = VectorIndex::open(&t.file());
        proptest::prop_assert!(v.dims() <= MAX_DIMS);
        if v.dims() > 0 {
            let _ = v.nearest(&vec![1.0; v.dims()], 5);
        }
    }

    #[test]
    fn nearest_is_the_best_by_an_exact_scan(vectors in proptest::collection::vec(proptest::collection::vec(-1.0f32..1.0, 6), 1..40), query in proptest::collection::vec(-1.0f32..1.0, 6)) {
        let mut v = VectorIndex::new(None);
        v.reset("m");
        for (i, x) in vectors.iter().enumerate() {
            let _ = v.insert(i as u64 + 1, 1, 0, x);
        }
        if let Ok(hits) = v.nearest(&query, 5) {
            proptest::prop_assert!(hits.len() <= 5);
            for w in hits.windows(2) {
                proptest::prop_assert!(w[0].similarity >= w[1].similarity);
            }
        }
    }
}
