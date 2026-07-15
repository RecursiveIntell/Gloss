use crate::candidate::CandidateList;
use crate::trait_def::ScoredCandidate;

#[test]
fn test_candidate_list_keeps_top_k() {
    let mut list = CandidateList::new(3);
    list.consider(0, 0.5);
    list.consider(1, 0.8);
    list.consider(2, 0.3);
    list.consider(3, 0.9);
    list.consider(4, 0.1);

    list.sort_descending();
    let sorted = list.sorted();

    assert_eq!(sorted.len(), 3);
    // Top 3 by score: 0.9 (idx 3), 0.8 (idx 1), 0.5 (idx 0)
    assert_eq!(sorted[0].idx, 3);
    assert_eq!(sorted[0].score, 0.9);
    assert_eq!(sorted[1].idx, 1);
    assert_eq!(sorted[2].idx, 0);
}

#[test]
fn test_candidate_list_empty() {
    let list = CandidateList::new(5);
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
}

#[test]
fn test_scored_candidate() {
    let c = ScoredCandidate::new(42, 0.95);
    assert_eq!(c.idx, 42);
    assert!((c.score - 0.95).abs() < 0.001);
}

#[test]
fn test_candidate_list_underfills() {
    let mut list = CandidateList::new(10);
    list.consider(0, 0.5);
    list.consider(1, 0.8);

    assert_eq!(list.len(), 2);
    assert!(!list.is_empty());
}
