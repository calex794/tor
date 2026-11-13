//! Pure, bounded ratio policy for the three authoritative shelf categories.
//! Stage REC-POLICY-01A: no persistence, no HTTP forms, no reader identity.
//! The third category is "other authors"; legacy feed wire value is "site".

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShelfShareError {
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShelfSharePolicy {
    /// [owner, active team, other authors]; sum must be exactly 100.
    with_team: [u8; 3],
    /// [owner, other authors] if there are no eligible team candidates.
    without_team: [u8; 2],
}

impl Default for ShelfSharePolicy {
    fn default() -> Self {
        Self {
            with_team: [50, 40, 10],
            without_team: [70, 30],
        }
    }
}

impl ShelfSharePolicy {
    pub fn validated(with_team: [i32; 3], without_team: [i32; 2]) -> Result<Self, ShelfShareError> {
        if with_team
            .iter()
            .chain(without_team.iter())
            .any(|v| !(0..=100).contains(v))
            || with_team.iter().sum::<i32>() != 100
            || without_team.iter().sum::<i32>() != 100
        {
            return Err(ShelfShareError::Invalid);
        }
        Ok(Self {
            with_team: with_team.map(|v| v as u8),
            without_team: without_team.map(|v| v as u8),
        })
    }

    pub const fn with_team(self) -> [u8; 3] {
        self.with_team
    }

    pub const fn without_team(self) -> [u8; 2] {
        self.without_team
    }

    /// Weights are targets across many independent shelves, not per five-card
    /// shelf guarantees. An administrator's explicit 0% must stay 0% even
    /// when every nonzero category has zero eligible candidates.
    pub fn weights_for_availability(self, available: [bool; 3]) -> [i64; 3] {
        let [author_exists, team_exists, other_exists] = available;
        if !team_exists {
            return [
                if author_exists {
                    i64::from(self.without_team[0])
                } else {
                    0
                },
                0,
                if other_exists {
                    i64::from(self.without_team[1])
                } else {
                    0
                },
            ];
        }
        let author = i64::from(self.with_team[0]);
        let teammate = i64::from(self.with_team[1]);
        let other = i64::from(self.with_team[2]);
        [
            if author_exists { author } else { 0 },
            teammate,
            if other_exists {
                // Existing default: with no owner Books, the 50% owner quota
                // transfers to the other-author source (40/60).
                // A configured 0% other-author quota, however, is a hard stop.
                other
                    + if !author_exists && other > 0 {
                        author
                    } else {
                        0
                    }
            } else {
                0
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::reader_end_shelf::{select_shelf_with_policy, ShelfCandidate};
    use std::collections::BTreeSet;

    #[test]
    fn defaults_are_504010_or_7030_and_legacy_fallbacks_remain() {
        let policy = ShelfSharePolicy::default();
        assert_eq!(policy.with_team(), [50, 40, 10]);
        assert_eq!(policy.without_team(), [70, 30]);
        assert_eq!(
            policy.weights_for_availability([true, true, true]),
            [50, 40, 10]
        );
        assert_eq!(
            policy.weights_for_availability([true, false, true]),
            [70, 0, 30]
        );
        assert_eq!(
            policy.weights_for_availability([false, true, true]),
            [0, 40, 60]
        );
        assert_eq!(
            policy.weights_for_availability([false, false, false]),
            [0, 0, 0]
        );
    }

    #[test]
    fn both_independent_ratios_must_equal_100_and_have_no_negative_inputs() {
        assert_eq!(
            ShelfSharePolicy::validated([50, 40, 10], [70, 30]),
            Ok(ShelfSharePolicy::default())
        );
        for (ordinary, without_team) in [
            ([50, 40, 9], [70, 30]),
            ([50, 40, 20], [70, 30]),
            ([40, 30, 30], [70, 31]),
            ([40, 30, 30], [-1, 101]),
            ([101, 0, -1], [60, 40]),
        ] {
            assert_eq!(
                ShelfSharePolicy::validated(ordinary, without_team),
                Err(ShelfShareError::Invalid)
            );
        }
    }

    fn pool(start: i64) -> Vec<ShelfCandidate> {
        (0..20)
            .map(|i| ShelfCandidate {
                book_id: start + i,
                relevance: 800,
                preference_boost: 0,
            })
            .collect()
    }

    fn tally(policy: &ShelfSharePolicy, team: bool) -> [usize; 3] {
        let mut counts = [0; 3];
        for rotation in 0..100 {
            let cards = select_shelf_with_policy(
                1,
                [pool(10), if team { pool(100) } else { vec![] }, pool(1000)],
                5,
                rotation,
                policy,
            );
            assert_eq!(cards.len(), 5);
            assert_eq!(
                cards
                    .iter()
                    .map(|c| c.book_id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                5
            );
            for card in cards {
                counts[card.source.index()] += 1;
            }
        }
        counts
    }

    #[test]
    fn configured_ratios_apply_over_rotations_without_per_shelf_rounding() {
        let policy = ShelfSharePolicy::validated([40, 30, 30], [60, 40]).unwrap();
        assert_eq!(tally(&policy, true), [200, 150, 150]);
        assert_eq!(tally(&policy, false), [300, 0, 200]);
    }

    #[test]
    fn explicit_zero_is_not_silently_backfilled_by_an_empty_category() {
        let policy = ShelfSharePolicy::validated([0, 100, 0], [0, 100]).unwrap();
        assert_eq!(
            policy.weights_for_availability([false, true, true]),
            [0, 100, 0]
        );
        assert_eq!(
            policy.weights_for_availability([true, false, false]),
            [0, 0, 0]
        );
        assert!(select_shelf_with_policy(1, [pool(10), vec![], vec![]], 5, 3, &policy,).is_empty());
        assert!(
            select_shelf_with_policy(1, [vec![], pool(100), vec![]], 5, 3, &policy,).len() == 5
        );
    }

    #[test]
    fn absent_owner_transfers_only_to_enabled_other_author_quota() {
        let policy = ShelfSharePolicy::validated([40, 30, 30], [60, 40]).unwrap();
        assert_eq!(
            policy.weights_for_availability([false, true, true]),
            [0, 30, 70]
        );
        let forbidden = ShelfSharePolicy::validated([40, 60, 0], [100, 0]).unwrap();
        assert_eq!(
            forbidden.weights_for_availability([false, true, true]),
            [0, 60, 0]
        );
    }
