//! RET-04: distinguish minimum viable inventory from full-card and exact-share inventory.
//!
//! These are pure allocator checks. SQL owner/team identity and discoverability
//! remain covered by the existing PostgreSQL/HTTP recommendation tests.
use std::collections::BTreeSet;
use tor_recommendations::services::reader_end_shelf::{select_shelf, ShelfCandidate, ShelfSource};

const SOURCE_BOOK: i64 = 1;

fn candidates(first: i64, count: usize) -> Vec<ShelfCandidate> {
    (0..count)
        .map(|index| ShelfCandidate {
            book_id: first + index as i64,
            relevance: 800,
            preference_boost: 0,
        })
        .collect()
}

fn tally_over_100_rotations(pools: [Vec<ShelfCandidate>; 3]) -> [usize; 3] {
    let mut totals = [0; 3];
    for rotation in 0..100 {
        let cards = select_shelf(SOURCE_BOOK, pools.clone(), 5, rotation);
        assert_eq!(cards.len(), 5, "rotation={rotation}");
        let ids = cards
            .iter()
            .map(|card| card.book_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 5, "duplicate Book at rotation={rotation}");
        assert!(
            !ids.contains(&SOURCE_BOOK),
            "source Book is not a candidate"
        );
        for card in cards {
            match card.source {
                ShelfSource::Author => totals[0] += 1,
                ShelfSource::Teammate => totals[1] += 1,
                ShelfSource::OtherAuthors => totals[2] += 1,
            }
        }
    }
    totals
}

#[test]
fn any_recommendation_needs_at_least_one_other_eligible_book() {
    assert!(select_shelf(SOURCE_BOOK, [vec![], vec![], vec![]], 5, 0).is_empty());

    // One owner, source Book plus another Book by the same owner.
    let owner_only = select_shelf(SOURCE_BOOK, [candidates(10, 1), vec![], vec![]], 5, 0);
    assert_eq!(owner_only.len(), 1);
    assert_eq!(owner_only[0].source, ShelfSource::Author);

    // Two owners, source owner's only Book plus one Book by another owner.
    let other_only = select_shelf(SOURCE_BOOK, [vec![], vec![], candidates(1000, 1)], 5, 0);
    assert_eq!(other_only.len(), 1);
    assert_eq!(other_only[0].source, ShelfSource::OtherAuthors);
}

#[test]
fn five_distinct_cards_require_five_eligible_books_but_not_three_sources() {
    // Source Book plus five other Books by the same owner: a full shelf,
    // although the nominal three-source distribution is impossible.
    let cards = select_shelf(SOURCE_BOOK, [candidates(10, 5), vec![], vec![]], 5, 37);
    assert_eq!(cards.len(), 5);
    assert!(cards.iter().all(|card| card.source == ShelfSource::Author));

    // Five candidates in all, distributed among two owners, also fill it.
    let mixed = select_shelf(
        SOURCE_BOOK,
        [candidates(10, 1), vec![], candidates(1000, 4)],
        5,
        37,
    );
    assert_eq!(mixed.len(), 5);
    assert_eq!(
        mixed
            .iter()
            .filter(|c| c.source == ShelfSource::Author)
            .count(),
        1
    );
    assert_eq!(
        mixed
            .iter()
            .filter(|c| c.source == ShelfSource::OtherAuthors)
            .count(),
        4
    );

    assert_eq!(
        select_shelf(SOURCE_BOOK, [candidates(10, 4), vec![], vec![]], 5, 0).len(),
        4
    );
}

#[test]
fn default_no_team_70_30_requires_four_owner_candidates_and_two_other_candidates() {
    // Minimal verified sufficient pool for exact 70/30 across 100 x 5 slots:
    // owner 5 Books including current, other-owner 2 Books; 2 owners, 7 Books total.
    assert_eq!(
        tally_over_100_rotations([candidates(10, 4), vec![], candidates(1000, 2)]),
        [350, 0, 150]
    );

    // One fewer owner Book still fills the shelf but cannot make 70%.
    assert_eq!(
        tally_over_100_rotations([candidates(10, 3), vec![], candidates(1000, 2)]),
        [300, 0, 200]
    );
}

#[test]
fn default_with_team_50_40_10_requires_four_owner_two_team_one_outside_candidates() {
    // Source owner: 5 Books including current (4 candidate Books).
    // Active teammate owner: 2 candidate Books.
    // Non-teammate owner: 1 candidate Book.
    // Therefore at least 3 owners, 8 Books total for this exact 5-slot
    // 100-rotation allocator benchmark, not a platform-wide author target.
    assert_eq!(
        tally_over_100_rotations([candidates(10, 4), candidates(100, 2), candidates(1000, 1),]),
        [250, 200, 50]
    );

    // Removing one owner candidate still fills five slots but changes the
    // aggregate ratios due to per-shelf depletion and reallocation.
    assert_eq!(
        tally_over_100_rotations([candidates(10, 3), candidates(100, 2), candidates(1000, 1),]),
        [250, 190, 60]
    );
}

fn tally_if_full(pools: [Vec<ShelfCandidate>; 3]) -> Option<[usize; 3]> {
    let mut totals = [0; 3];
    for rotation in 0..100 {
        let cards = select_shelf(SOURCE_BOOK, pools.clone(), 5, rotation);
        if cards.len() != 5 {
            return None;
        }
        for card in cards {
            match card.source {
                ShelfSource::Author => totals[0] += 1,
                ShelfSource::Teammate => totals[1] += 1,
                ShelfSource::OtherAuthors => totals[2] += 1,
            }
        }
    }
    Some(totals)
}

#[test]
fn smaller_eligible_inventories_cannot_sustain_exact_shares_over_100_rotations() {
    // The exact 70/30 benchmark needs at least 6 candidates in total:
    // 4 more Books by the source owner + 2 outside Books.
    for author in 1..=5 {
        for other in 1..=5 {
            if author + other < 6 {
                assert_ne!(
                    tally_if_full([candidates(10, author), vec![], candidates(1000, other)]),
                    Some([350, 0, 150]),
                    "author={author} other={other}"
                );
            }
        }
    }

    // All three source categories need eligible candidates. The exact
    // 50/40/10 benchmark needs at least 7 candidates in total:
    // 4 more owner Books + 2 active teammate Books + 1 outside Book.
    for author in 1..=6 {
        for team in 1..=6 {
            for other in 1..=6 {
                if author + team + other < 7 {
                    assert_ne!(
                        tally_if_full([
                            candidates(10, author),
                            candidates(100, team),
                            candidates(1000, other),
                        ]),
                        Some([250, 200, 50]),
                        "author={author} team={team} other={other}"
                    );
                }
            }
        }
    }
}
