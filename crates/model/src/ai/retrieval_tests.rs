// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::retrieval::*;

#[test]
fn a_request_is_checked_and_its_limit_capped() {
    let r = RetrievalRequest::new("What deposit did Harbor Hall ask for?", None, None, None, None).unwrap();
    assert_eq!(r.limit, DEFAULT_LIMIT);
    assert_eq!(RetrievalRequest::new("x", Some(u64::MAX), None, None, None).unwrap().limit, MAX_LIMIT);
    assert_eq!(RetrievalRequest::new("x", Some(3), Some(" ada@example.org "), Some(4), Some(5)).unwrap().from.as_deref(), Some("ada@example.org"));
    assert!(matches!(RetrievalRequest::new("x", Some(0), None, None, None), Err(AiError::Invalid(_))));
    assert!(matches!(RetrievalRequest::new("x", None, Some("  "), None, None), Err(AiError::Invalid(_))));
    assert!(matches!(RetrievalRequest::new("x", None, Some("a".repeat(400).as_str()), None, None), Err(AiError::Invalid(_))));
    assert!(matches!(RetrievalRequest::new("x", None, None, Some(0), None), Err(AiError::Invalid(_))));
    assert!(matches!(RetrievalRequest::new(&"q".repeat(MAX_QUERY_CHARS + 1), None, None, None, None), Err(AiError::Invalid(_))));
    // An empty query is a request (it finds nothing).
    assert!(RetrievalRequest::new("", None, None, None, None).is_ok());
}

#[test]
fn fusion_adds_reciprocal_ranks() {
    let fused = fuse(&[vec![1, 2, 3], vec![3, 1]], RRF_K);
    let ids: Vec<u64> = fused.iter().map(|(id, _)| *id).collect();
    // 1: 1/61 + 1/62; 3: 1/63 + 1/61; 2: 1/62.
    assert_eq!(ids, [1, 3, 2]);
    let one = fused.iter().find(|(id, _)| *id == 1).map(|(_, s)| *s).unwrap();
    assert!((one - (1.0 / 61.0 + 1.0 / 62.0)).abs() < 1e-12);
}

#[test]
fn fusion_counts_a_message_once_a_list_and_breaks_ties_by_id() {
    let fused = fuse(&[vec![5, 5, 5, 9]], RRF_K);
    assert_eq!(fused.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [5, 9]);
    let tie = fuse(&[vec![8], vec![4]], RRF_K);
    assert_eq!(tie.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [4, 8]);
    assert!(fuse(&[], RRF_K).is_empty());
    assert_eq!(fuse(&[vec![1]], 0), vec![(1, 1.0)]);
}

proptest::proptest! {
    #[test]
    fn fusion_ignores_the_order_of_the_lists_and_never_grows(
        lists in proptest::collection::vec(proptest::collection::vec(0u64..50, 0..30), 0..6),
    ) {
        let a = fuse(&lists, RRF_K);
        let mut reversed = lists.clone();
        reversed.reverse();
        let b = fuse(&reversed, RRF_K);
        // The same score for each message (the sums may round in another order).
        let b: std::collections::HashMap<u64, f64> = b.into_iter().collect();
        for (id, score) in &a {
            proptest::prop_assert!(b.get(id).is_some_and(|s| (s - score).abs() < 1e-9));
        }
        // Each message once, no more than all the lists hold, best first.
        let distinct: std::collections::HashSet<u64> = lists.iter().flatten().copied().collect();
        proptest::prop_assert_eq!(a.len(), distinct.len());
        for w in a.windows(2) {
            proptest::prop_assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn a_message_in_one_more_list_never_scores_lower(
        lists in proptest::collection::vec(proptest::collection::vec(0u64..20, 1..10), 1..5),
        extra in 0u64..20,
    ) {
        let before = fuse(&lists, RRF_K).into_iter().find(|(id, _)| *id == extra).map_or(0.0, |(_, s)| s);
        let mut more = lists.clone();
        more.push(vec![extra]);
        let after = fuse(&more, RRF_K).into_iter().find(|(id, _)| *id == extra).map_or(0.0, |(_, s)| s);
        proptest::prop_assert!(after > before);
    }
}
