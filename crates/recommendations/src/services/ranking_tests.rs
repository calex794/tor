use super::*;
fn seed_two() -> RankingSnapshot {
    let (s, st) = RankingSnapshot::empty(7)
        .unwrap()
        .start_verified_placement(20)
        .unwrap();
    assert_eq!(st, AnswerStatus::Ranked);
    let (s, st) = s.start_verified_placement(30).unwrap();
    assert_eq!(st, AnswerStatus::Placing);
    s
}
#[test]
fn first_book_seeds_without_question_and_second_is_canonical() {
    let s = seed_two();
    let q = s.next_question().unwrap().unwrap();
    assert_eq!((q.book_a_id, q.book_b_id), (20, 30));
    assert!(q.comparison_id.starts_with("book-compare-v1:"));
    assert_eq!(q.comparison_id, comparison_id(7, 20, 30, q.state_version));
}
#[test]
fn winning_and_losing_ranks_do_not_mutate_the_original_snapshot() {
    let s = seed_two();
    let q = s.next_question().unwrap().unwrap();
    let (ahead, status) = s
        .answer(q.state_version, &q.comparison_id, Outcome::BWins)
        .unwrap();
    assert_eq!(status, AnswerStatus::Ranked);
    assert_eq!(
        ahead
            .sorted_ranks()
            .iter()
            .map(|r| r.book_id)
            .collect::<Vec<_>>(),
        vec![30, 20]
    );
    assert_eq!(
        ahead
            .ranks
            .iter()
            .find(|r| r.book_id == 20)
            .unwrap()
            .comparisons_count,
        1
    );
    assert_eq!(s.ranks.len(), 1);
    let (behind, _) = s
        .answer(q.state_version, &q.comparison_id, Outcome::AWins)
        .unwrap();
    assert_eq!(
        behind
            .sorted_ranks()
            .iter()
            .map(|r| r.book_id)
            .collect::<Vec<_>>(),
        vec![20, 30]
    );
    assert_eq!(
        ahead
            .ranks
            .iter()
            .find(|r| r.book_id == 30)
            .unwrap()
            .confidence(),
        0.5
    );
}
#[test]
fn tie_group_preserves_shared_tier_and_deterministic_question() {
    let s = seed_two();
    let q = s.next_question().unwrap().unwrap();
    let (mut tied, status) = s
        .answer(q.state_version, &q.comparison_id, Outcome::Tie)
        .unwrap();
    assert_eq!(status, AnswerStatus::Ranked);
    assert_eq!(tied.ranks[0].tier, tied.ranks[1].tier);
    let (s, st) = tied.start_verified_placement(5).unwrap();
    tied = s;
    assert_eq!(st, AnswerStatus::Placing);
    let q = tied.next_question().unwrap().unwrap();
    assert_eq!(q.target_book_id, 20);
    assert_eq!(q.book_a_id, 20); // string BookReference order: "20" < "5".
}
#[test]
fn skip_is_sticky_until_explicit_resume_and_stale_answer_is_rejected() {
    let s = seed_two();
    let q = s.next_question().unwrap().unwrap();
    let (skipped, st) = s
        .answer(q.state_version, &q.comparison_id, Outcome::Skip)
        .unwrap();
    assert_eq!(st, AnswerStatus::Skipped);
    assert!(skipped.next_question().unwrap().is_none());
    assert_eq!(
        skipped.answer(q.state_version, &q.comparison_id, Outcome::AWins),
        Err(RankingError::Exhausted)
    );
    assert!(skipped
        .start_verified_placement(30)
        .unwrap()
        .0
        .next_question()
        .unwrap()
        .is_none());
    let resumed = skipped.resume_exhausted_skips().unwrap();
    let new_q = resumed.next_question().unwrap().unwrap();
    assert_ne!(new_q.comparison_id, q.comparison_id);
    assert_eq!(
        resumed.answer(q.state_version, &q.comparison_id, Outcome::AWins),
        Err(RankingError::Stale)
    );
}
#[test]
fn deferring_a_ranked_target_removes_it_without_fabricating_next_read() {
    let s = seed_two();
    let q = s.next_question().unwrap().unwrap();
    let (s, st) = s
        .answer(q.state_version, &q.comparison_id, Outcome::DeferA)
        .unwrap();
    assert_eq!(st, AnswerStatus::Deferred(20));
    assert!(s.ranks.is_empty());
    assert_eq!(s.start_verified_placement(20), Err(RankingError::Invalid));
    assert!(s
        .release_deferred_after_verified_read(20)
        .unwrap()
        .deferred_books
        .is_empty());
    assert!(s.placement.is_none());
}
#[test]
fn placement_requires_valid_distinct_ranks_and_single_active_candidate() {
    let s = seed_two();
    assert_eq!(s.start_verified_placement(40), Err(RankingError::Conflict));
    let mut broken = s.clone();
    broken.ranks.push(broken.ranks[0].clone());
    assert_eq!(broken.validate(), Err(RankingError::Invalid));
    assert_eq!(RankingSnapshot::empty(0), Err(RankingError::Invalid));
    assert_eq!(s.start_verified_placement(-1), Err(RankingError::Invalid));
}
#[test]
fn median_binary_placement_handles_multiple_existing_tiers() {
    let mut s = RankingSnapshot::empty(7).unwrap();
    for (id, tier) in [
        (1, -2 * TIER_STEP),
        (2, -TIER_STEP),
        (3, 0),
        (4, TIER_STEP),
        (5, 2 * TIER_STEP),
    ] {
        s.ranks.push(BookRank {
            book_id: id,
            tier,
            comparisons_count: 0,
        });
    }
    let (s, _) = s.start_verified_placement(9).unwrap();
    let q = s.next_question().unwrap().unwrap();
    assert_eq!(q.target_book_id, 3);
    let (s, st) = s
        .answer(q.state_version, &q.comparison_id, Outcome::BWins)
        .unwrap();
    assert_eq!(st, AnswerStatus::Placing);
    let q = s.next_question().unwrap().unwrap();
    assert_eq!(q.target_book_id, 1); // positions 0..1: midpoint 0
}
#[test]
fn sparse_tiers_rebalance_without_losing_ties() {
    let mut ranks = vec![
        BookRank {
            book_id: 1,
            tier: 0,
            comparisons_count: 0,
        },
        BookRank {
            book_id: 2,
            tier: 1,
            comparisons_count: 0,
        },
        BookRank {
            book_id: 3,
            tier: 1,
            comparisons_count: 0,
        },
    ];
    let new_tier = tier_for_insert(&mut ranks, 1).unwrap();
    assert!(ranks[0].tier < new_tier && new_tier < ranks[1].tier);
    assert_eq!(ranks[1].tier, ranks[2].tier);
}
#[test]
fn ranking_is_strictly_personal_no_global_book_popularity() {
    let s = seed_two();
    let mut other = s.clone();
    other.user_id = 8;
    let first = s.next_question().unwrap().unwrap();
    let second = other.next_question().unwrap().unwrap();
    assert_ne!(first.comparison_id, second.comparison_id);
    assert_eq!(
        other.answer(first.state_version, &first.comparison_id, Outcome::AWins),
        Err(RankingError::Stale)
    );
}
