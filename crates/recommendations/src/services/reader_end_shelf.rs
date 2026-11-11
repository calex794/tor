//! Reader-end shelf allocator (F / REC).
//!
//! This module does not query a catalogue, authorize a Book, collect a reader
//! history, or charge an advertising account. Callers MUST supply only currently
//! discoverable Books, grouped by their authoritative owner/team relationships.
//! The allocator chooses up to five DISTINCT cards after a free chapter.
use std::collections::BTreeSet;

/// Owner of the current publication takes precedence over team membership;
/// team membership takes precedence over the rest of the public catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShelfSource {
    Author,
    Teammate,
    Site,
}

impl ShelfSource {
    pub const fn index(self) -> usize {
        match self {
            Self::Author => 0,
            Self::Teammate => 1,
            Self::Site => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfCandidate {
    pub book_id: i64,
    /// Score produced by the existing REC recommendation model.
    pub relevance: i64,
    /// Bounded explicit-preference/context adjustment; no history required.
    pub preference_boost: i64,
}

impl ShelfCandidate {
    fn score(&self) -> i64 {
        self.relevance.clamp(0, 1000) + self.preference_boost.clamp(0, 200)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfCard {
    pub book_id: i64,
    pub source: ShelfSource,
}

const SOURCES: [ShelfSource; 3] = [
    ShelfSource::Author,
    ShelfSource::Teammate,
    ShelfSource::Site,
];
const MAX_POOL: usize = 100;

/// Nominal ratio 50/40/10. If no teammate Books exist, the *product-defined*
/// fallback is 70/0/30 (not mathematical proportional redistribution).
/// With no author Books, author inventory transfers to the site. If the site
/// is empty, the remaining author/team shares retain their 5:4 ratio.
pub fn weights_for_availability(available: [bool; 3]) -> [i64; 3] {
    match available {
        [true, true, true] => [50, 40, 10],
        [true, false, true] => [70, 0, 30],
        [false, true, true] => [0, 40, 60],
        [true, true, false] => [5, 4, 0],
        [true, false, false] => [1, 0, 0],
        [false, true, false] => [0, 1, 0],
        [false, false, true] => [0, 0, 1],
        [false, false, false] => [0, 0, 0],
    }
}

fn next_source(weights: [i64; 3], debt: &mut [i64; 3]) -> Option<usize> {
    let sum: i64 = weights.iter().sum();
    if sum == 0 {
        return None;
    }
    for i in 0..3 {
        if weights[i] == 0 {
            debt[i] = 0;
        } else {
            debt[i] += weights[i];
        }
    }
    let selected = (0..3)
        .filter(|&i| weights[i] != 0)
        .max_by_key(|&i| (debt[i], -(i as i64)))?;
    debt[selected] -= sum;
    Some(selected)
}

fn order_pool(pool: &mut [ShelfCandidate], rotation: u64) {
    pool.sort_by(|a, b| {
        b.score()
            .cmp(&a.score())
            // A stable variation among *equally scored* Books avoids the
            // same tie ordering on every chapter without weakening relevance.
            .then_with(|| {
                let ka = (a.book_id as u64 ^ rotation).wrapping_mul(0x9e3779b97f4a7c15);
                let kb = (b.book_id as u64 ^ rotation).wrapping_mul(0x9e3779b97f4a7c15);
                ka.cmp(&kb)
            })
            .then(a.book_id.cmp(&b.book_id))
    });
}

/// Select 4 or 5 cards. No placeholder is fabricated when the union is small.
/// A repeated Book is assigned to its strongest relationship category only.
/// `rotation` is a stable bounded request/chapter variation supplied by the
/// caller; the function is pure and does not store per-reader observations.
pub fn select_shelf(
    source_book_id: i64,
    mut pools: [Vec<ShelfCandidate>; 3],
    cards: usize,
    rotation: u64,
) -> Vec<ShelfCard> {
    if source_book_id <= 0 || !(4..=5).contains(&cards) {
        return Vec::new();
    }

    let mut seen = BTreeSet::from([source_book_id]);
    for pool in &mut pools {
        order_pool(pool, rotation);
        pool.retain(|item| item.book_id > 0 && seen.insert(item.book_id));
        pool.truncate(MAX_POOL);
        // Every Book has exactly one slot. Candidates must have been checked
        // for public Book visibility and viewer age permissions by the caller.
    }

    let mut debt = [0_i64; 3];
    let initial = weights_for_availability(pools.clone().map(|p| !p.is_empty()));
    // Advance the smooth weighted wheel before emitting a shelf. Different
    // chapter/request rotations therefore realize fractional 10% site share
    // even when one shelf has only five positions.
    for _ in 0..rotation % 100 {
        let _ = next_source(initial, &mut debt);
    }

    let mut output = Vec::with_capacity(cards);
    for _ in 0..cards {
        let available = pools.each_ref().map(|pool| !pool.is_empty());
        let weights = weights_for_availability(available);
        let Some(index) = next_source(weights, &mut debt) else {
            break;
        };
        let candidate = pools[index].remove(0);
        output.push(ShelfCard {
            book_id: candidate.book_id,
            source: SOURCES[index],
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(start: i64, count: usize) -> Vec<ShelfCandidate> {
        (0..count)
            .map(|i| ShelfCandidate {
                book_id: start + i as i64,
                relevance: 800 - i as i64,
                preference_boost: 0,
            })
            .collect()
    }

    fn count_across_rotations(pools: [Vec<ShelfCandidate>; 3]) -> [usize; 3] {
        let mut counts = [0; 3];
        for rotation in 0..100 {
            let cards = select_shelf(1, pools.clone(), 5, rotation);
            assert_eq!(cards.len(), 5);
            let mut ids = BTreeSet::new();
            for card in cards {
                assert!(ids.insert(card.book_id));
                counts[card.source.index()] += 1;
            }
        }
        counts
    }

    #[test]
    fn nominal_50_40_10_balances_fractional_site_position() {
        assert_eq!(
            count_across_rotations([pool(10, 20), pool(100, 20), pool(1000, 20)]),
            [250, 200, 50]
        );
    }

    #[test]
    fn no_team_gives_70_30_not_83_17() {
        assert_eq!(
            count_across_rotations([pool(10, 20), vec![], pool(1000, 20)]),
            [350, 0, 150]
        );
    }

    #[test]
    fn no_author_transfers_its_share_to_site() {
        assert_eq!(
            count_across_rotations([vec![], pool(100, 20), pool(1000, 20)]),
            [0, 200, 300]
        );
    }

    #[test]
    fn partial_shortage_never_duplicates_or_leaves_unnecessary_holes() {
        for rotation in 0..20 {
            let cards = select_shelf(1, [pool(10, 1), vec![], pool(1000, 20)], 5, rotation);
            assert_eq!(cards.len(), 5);
            assert_eq!(
                cards
                    .iter()
                    .filter(|c| c.source == ShelfSource::Author)
                    .count(),
                1
            );
            assert_eq!(
                cards
                    .iter()
                    .map(|c| c.book_id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                5
            );
        }
    }

    #[test]
    fn duplicates_are_owned_by_author_before_team_before_site() {
        let cards = select_shelf(1, [pool(2, 1), pool(2, 2), pool(2, 3)], 5, 2);
        assert_eq!(cards.len(), 3);
        assert_eq!(
            cards
                .iter()
                .find(|card| card.book_id == 2)
                .map(|c| c.source),
            Some(ShelfSource::Author)
        );
        assert_eq!(
            cards
                .iter()
                .find(|card| card.book_id == 3)
                .map(|c| c.source),
            Some(ShelfSource::Teammate)
        );
    }

    #[test]
    fn personal_preference_reranks_only_within_an_authoritative_pool() {
        let cards = select_shelf(
            1,
            [
                vec![
                    ShelfCandidate {
                        book_id: 2,
                        relevance: 800,
                        preference_boost: 0,
                    },
                    ShelfCandidate {
                        book_id: 3,
                        relevance: 730,
                        preference_boost: 150,
                    },
                ],
                vec![],
                vec![],
            ],
            4,
            0,
        );
        assert_eq!(
            cards.iter().map(|item| item.book_id).collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert!(cards.iter().all(|item| item.source == ShelfSource::Author));
    }

    #[test]
    fn no_ineligible_placeholder_or_unbounded_requested_shelf() {
        assert!(select_shelf(1, [vec![], vec![], vec![]], 5, 0).is_empty());
        assert!(select_shelf(1, [pool(10, 5), vec![], vec![]], 10, 0).is_empty());
        assert_eq!(
            select_shelf(1, [pool(10, 8), vec![], vec![]], 4, 0).len(),
            4
        );
    }
}
