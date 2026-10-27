//! F-05 / REC-08: deterministic personal pairwise ranking.
//! Pure transition rules ported from NEwBOO user_book_ranking_service.py.
//! Eligibility is NOT inferred from a bookmark or a book-view event: callers
//! must verify a real completed chapter before starting any placement.
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const TIER_STEP: i64 = 1_i64 << 32;
pub const MAX_RANKED_BOOKS: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookRank {
    pub book_id: i64,
    pub tier: i64,
    pub comparisons_count: u64,
}

impl BookRank {
    pub fn confidence(&self) -> f64 {
        let n = self.comparisons_count as f64;
        if n == 0.0 {
            0.0
        } else {
            n / (n + 1.0)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementState {
    pub placing_book_id: i64,
    pub low_position: i32,
    pub high_position: i32,
    pub state_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub comparison_id: String,
    pub book_a_id: i64,
    pub book_b_id: i64,
    pub placing_book_id: i64,
    pub target_book_id: i64,
    pub state_version: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    AWins,
    BWins,
    Tie,
    Skip,
    DeferA,
    DeferB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerStatus {
    Ranked,
    Placing,
    Skipped,
    Deferred(i64),
    AlreadyRanked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankingError {
    Invalid,
    Conflict,
    Stale,
    Exhausted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankingSnapshot {
    pub user_id: i64,
    pub ranks: Vec<BookRank>,
    pub placement: Option<PlacementState>,
    /// Skip markers apply to one placement window, not to all later questions.
    pub skipped_pairs: BTreeSet<(i64, i64)>,
    /// Durable defer markers: a fresh verified read is required to release one.
    pub deferred_books: BTreeSet<i64>,
    /// Monotone across placements to invalidate old questions.
    pub revision: u64,
}

fn canonical_pair(a: i64, b: i64) -> Result<(i64, i64), RankingError> {
    if a <= 0 || b <= 0 || a == b {
        return Err(RankingError::Invalid);
    }
    if a.to_string() <= b.to_string() {
        Ok((a, b))
    } else {
        Ok((b, a))
    }
}

/// Exactly NEwBOO's "v1:user:a:b:placement:version" SHA-256 question ID.
fn comparison_id(user_id: i64, a: i64, b: i64, version: u64) -> String {
    let raw = format!("v1:{user_id}:{a}:{b}:placement:{version}");
    let hash = Sha256::digest(raw.as_bytes());
    format!("book-compare-v1:{hash:x}")
}

fn tier_groups(ranks: &[BookRank]) -> Vec<(i64, Vec<i64>)> {
    let mut groups: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for rank in ranks {
        groups.entry(rank.tier).or_default().push(rank.book_id);
    }
    groups
        .into_iter()
        .map(|(tier, mut ids)| {
            ids.sort_by_key(ToString::to_string); // Match Python's string BookReference ordering.
            (tier, ids)
        })
        .collect()
}

fn tier_for_insert(ranks: &mut [BookRank], position: usize) -> Result<i64, RankingError> {
    let groups = tier_groups(ranks);
    if position > groups.len() {
        return Err(RankingError::Invalid);
    }
    let tier = if groups.is_empty() {
        Some(0)
    } else if position == 0 {
        groups[0].0.checked_sub(TIER_STEP)
    } else if position == groups.len() {
        groups[groups.len() - 1].0.checked_add(TIER_STEP)
    } else {
        let lower = groups[position - 1].0;
        let upper = groups[position].0;
        ((upper as i128 - lower as i128) > 1)
            .then_some((lower as i128 + (upper as i128 - lower as i128) / 2) as i64)
    };
    if let Some(tier) = tier {
        return Ok(tier);
    }
    // A rare full rebalance is preferable to precision loss or tier overflow.
    let origin = -(i64::try_from(groups.len()).map_err(|_| RankingError::Invalid)? / 2) * TIER_STEP;
    let new_tiers: BTreeMap<_, _> = groups
        .iter()
        .enumerate()
        .map(|(i, (old, _))| (*old, origin + i as i64 * TIER_STEP))
        .collect();
    for rank in ranks.iter_mut() {
        rank.tier = *new_tiers.get(&rank.tier).ok_or(RankingError::Invalid)?;
    }
    let repaired = tier_groups(ranks);
    if position == 0 {
        return Ok(repaired[0].0 - TIER_STEP);
    }
    if position == repaired.len() {
        return Ok(repaired.last().ok_or(RankingError::Invalid)?.0 + TIER_STEP);
    }
    Ok(repaired[position - 1].0 + (repaired[position].0 - repaired[position - 1].0) / 2)
}

impl RankingSnapshot {
    pub fn empty(user_id: i64) -> Result<Self, RankingError> {
        if user_id <= 0 {
            return Err(RankingError::Invalid);
        }
        Ok(Self {
            user_id,
            ranks: Vec::new(),
            placement: None,
            skipped_pairs: BTreeSet::new(),
            deferred_books: BTreeSet::new(),
            revision: 0,
        })
    }

    pub fn validate(&self) -> Result<(), RankingError> {
        if self.user_id <= 0 || self.ranks.len() > MAX_RANKED_BOOKS {
            return Err(RankingError::Invalid);
        }
        let mut ids = BTreeSet::new();
        for rank in &self.ranks {
            if rank.book_id <= 0 || !ids.insert(rank.book_id) {
                return Err(RankingError::Invalid);
            }
        }
        if let Some(s) = &self.placement {
            let groups = tier_groups(&self.ranks);
            if s.placing_book_id <= 0
                || ids.contains(&s.placing_book_id)
                || s.state_version == 0
                || s.low_position < 0
                || s.high_position < s.low_position
                || s.high_position as usize >= groups.len()
            {
                return Err(RankingError::Invalid);
            }
        } else if !self.skipped_pairs.is_empty() {
            return Err(RankingError::Invalid);
        }
        Ok(())
    }

    pub fn sorted_ranks(&self) -> Vec<BookRank> {
        let mut result = self.ranks.clone();
        result.sort_by(|a, b| {
            a.tier
                .cmp(&b.tier)
                .then_with(|| a.book_id.to_string().cmp(&b.book_id.to_string()))
        });
        result
    }

    pub fn next_question(&self) -> Result<Option<Question>, RankingError> {
        self.validate()?;
        let Some(s) = &self.placement else {
            return Ok(None);
        };
        let groups = tier_groups(&self.ranks);
        let mid = (s.low_position + s.high_position) / 2;
        let mut positions = (s.low_position..=s.high_position).collect::<Vec<_>>();
        positions.sort_by_key(|n| ((i64::from(*n) - i64::from(mid)).abs(), *n));
        for position in positions {
            for target_id in &groups[position as usize].1 {
                let (a, b) = canonical_pair(s.placing_book_id, *target_id)?;
                if self.skipped_pairs.contains(&(a, b)) {
                    continue;
                }
                return Ok(Some(Question {
                    comparison_id: comparison_id(self.user_id, a, b, s.state_version),
                    book_a_id: a,
                    book_b_id: b,
                    placing_book_id: s.placing_book_id,
                    target_book_id: *target_id,
                    state_version: s.state_version,
                }));
            }
        }
        Ok(None)
    }

    /// Start only AFTER the caller has independently verified a trusted
    /// completed chapter belonging to this authenticated user.
    pub fn start_verified_placement(
        &self,
        book_id: i64,
    ) -> Result<(Self, AnswerStatus), RankingError> {
        self.validate()?;
        if book_id <= 0 || self.deferred_books.contains(&book_id) {
            return Err(RankingError::Invalid);
        }
        if self.ranks.iter().any(|rank| rank.book_id == book_id) {
            return Ok((self.clone(), AnswerStatus::AlreadyRanked));
        }
        if let Some(s) = &self.placement {
            if s.placing_book_id == book_id {
                return Ok((self.clone(), AnswerStatus::Placing));
            }
            return Err(RankingError::Conflict);
        }
        if self.ranks.len() >= MAX_RANKED_BOOKS {
            return Err(RankingError::Invalid);
        }
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        if next.ranks.is_empty() {
            next.ranks.push(BookRank {
                book_id,
                tier: 0,
                comparisons_count: 0,
            });
            return Ok((next, AnswerStatus::Ranked));
        }
        next.placement = Some(PlacementState {
            placing_book_id: book_id,
            low_position: 0,
            high_position: i32::try_from(tier_groups(&next.ranks).len())
                .map_err(|_| RankingError::Invalid)?
                - 1,
            state_version: 1,
        });
        next.skipped_pairs.clear();
        Ok((next, AnswerStatus::Placing))
    }

    /// A new *verified* reading event may release a previously deferred book.
    /// Callers must check event time > deferred_at in durable storage.
    pub fn release_deferred_after_verified_read(&self, book_id: i64) -> Result<Self, RankingError> {
        if book_id <= 0 {
            return Err(RankingError::Invalid);
        }
        let mut next = self.clone();
        next.deferred_books.remove(&book_id);
        Ok(next)
    }

    /// Skip exhaustion is sticky across refresh, until the user explicitly resumes.
    pub fn resume_exhausted_skips(&self) -> Result<Self, RankingError> {
        if self.placement.is_none()
            || self.next_question()?.is_some()
            || self.skipped_pairs.is_empty()
        {
            return Err(RankingError::Conflict);
        }
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        let state = next.placement.as_mut().ok_or(RankingError::Invalid)?;
        state.state_version = state
            .state_version
            .checked_add(1)
            .ok_or(RankingError::Invalid)?;
        next.skipped_pairs.clear();
        Ok(next)
    }

    /// Compute a complete next snapshot; a PostgreSQL adapter must commit this
    /// with an atomic expected_state_version compare-and-swap.
    pub fn answer(
        &self,
        expected_state_version: u64,
        supplied_id: &str,
        outcome: Outcome,
    ) -> Result<(Self, AnswerStatus), RankingError> {
        self.validate()?;
        let state = self.placement.as_ref().ok_or(RankingError::Stale)?;
        if state.state_version != expected_state_version {
            return Err(RankingError::Stale);
        }
        let q = self.next_question()?.ok_or(RankingError::Exhausted)?;
        if q.comparison_id != supplied_id {
            return Err(RankingError::Stale);
        }
        let mut next = self.clone();
        if outcome == Outcome::Skip {
            next.skipped_pairs
                .insert(canonical_pair(q.book_a_id, q.book_b_id)?);
            return Ok((next, AnswerStatus::Skipped));
        }
        if matches!(outcome, Outcome::DeferA | Outcome::DeferB) {
            let id = if outcome == Outcome::DeferA {
                q.book_a_id
            } else {
                q.book_b_id
            };
            next.ranks.retain(|rank| rank.book_id != id);
            next.deferred_books.insert(id);
            next.placement = None;
            next.skipped_pairs.clear();
            next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
            return Ok((next, AnswerStatus::Deferred(id)));
        }
        let target_index = next
            .ranks
            .iter()
            .position(|rank| rank.book_id == q.target_book_id)
            .ok_or(RankingError::Invalid)?;
        next.ranks[target_index].comparisons_count = next.ranks[target_index]
            .comparisons_count
            .checked_add(1)
            .ok_or(RankingError::Invalid)?;
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        if outcome == Outcome::Tie {
            let same_tier = next.ranks[target_index].tier;
            next.ranks.push(BookRank {
                book_id: q.placing_book_id,
                tier: same_tier,
                comparisons_count: state.state_version,
            });
            next.placement = None;
            next.skipped_pairs.clear();
            return Ok((next, AnswerStatus::Ranked));
        }
        let group_position = tier_groups(&self.ranks)
            .iter()
            .position(|(_, ids)| ids.contains(&q.target_book_id))
            .ok_or(RankingError::Invalid)? as i32;
        let winner_id = if outcome == Outcome::AWins {
            q.book_a_id
        } else {
            q.book_b_id
        };
        let mut low = state.low_position;
        let mut high = state.high_position;
        if winner_id == state.placing_book_id {
            high = group_position - 1;
        } else {
            low = group_position + 1;
        }
        if low <= high {
            next.placement = Some(PlacementState {
                placing_book_id: state.placing_book_id,
                low_position: low,
                high_position: high,
                state_version: state
                    .state_version
                    .checked_add(1)
                    .ok_or(RankingError::Invalid)?,
            });
            next.skipped_pairs.clear();
            return Ok((next, AnswerStatus::Placing));
        }
        let tier = tier_for_insert(&mut next.ranks, low as usize)?;
        next.ranks.push(BookRank {
            book_id: state.placing_book_id,
            tier,
            comparisons_count: state.state_version,
        });
        next.placement = None;
        next.skipped_pairs.clear();
        Ok((next, AnswerStatus::Ranked))
    }
}

#[cfg(test)]
#[path = "ranking_tests.rs"]
mod tests;
