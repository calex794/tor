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
        if n == 0.0 { 0.0 } else { n / (n + 1.0) }
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
pub enum Outcome { AWins, BWins, Tie, Skip, DeferA, DeferB }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerStatus { Ranked, Placing, Skipped, Deferred(i64), AlreadyRanked }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankingError { Invalid, Conflict, Stale, Exhausted }

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
    if a <= 0 || b <= 0 || a == b { return Err(RankingError::Invalid); }
    if a.to_string() <= b.to_string() { Ok((a, b)) } else { Ok((b, a)) }
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
    groups.into_iter().map(|(tier, mut ids)| {
        ids.sort_by_key(ToString::to_string); // Match Python's string BookReference ordering.
        (tier, ids)
    }).collect()
}

fn tier_for_insert(ranks: &mut [BookRank], position: usize) -> Result<i64, RankingError> {
    let groups = tier_groups(ranks);
    if position > groups.len() { return Err(RankingError::Invalid); }
    let tier = if groups.is_empty() {
        Some(0)
    } else if position == 0 {
        groups[0].0.checked_sub(TIER_STEP)
    } else if position == groups.len() {
        groups[groups.len() - 1].0.checked_add(TIER_STEP)
    } else {
        let lower = groups[position - 1].0;
        let upper = groups[position].0;
        (upper - lower > 1).then_some(lower + (upper - lower) / 2)
    };
    if let Some(tier) = tier { return Ok(tier); }
    // A rare full rebalance is preferable to precision loss or tier overflow.
    let origin = -(i64::try_from(groups.len()).map_err(|_| RankingError::Invalid)? / 2) * TIER_STEP;
    let new_tiers: BTreeMap<_, _> = groups.iter().enumerate()
        .map(|(i, (old, _))| (*old, origin + i as i64 * TIER_STEP)).collect();
    for rank in ranks.iter_mut() {
        rank.tier = *new_tiers.get(&rank.tier).ok_or(RankingError::Invalid)?;
    }
    let repaired = tier_groups(ranks);
    if position == 0 { return Ok(repaired[0].0 - TIER_STEP); }
    if position == repaired.len() { return Ok(repaired.last().ok_or(RankingError::Invalid)?.0 + TIER_STEP); }
    Ok(repaired[position - 1].0 + (repaired[position].0 - repaired[position - 1].0) / 2)
}

impl RankingSnapshot {
    pub fn empty(user_id: i64) -> Result<Self, RankingError> {
        if user_id <= 0 { return Err(RankingError::Invalid); }
        Ok(Self {
            user_id, ranks: Vec::new(), placement: None,
            skipped_pairs: BTreeSet::new(), deferred_books: BTreeSet::new(), revision: 0,
        })
    }

    pub fn validate(&self) -> Result<(), RankingError> {
        if self.user_id <= 0 || self.ranks.len() > MAX_RANKED_BOOKS { return Err(RankingError::Invalid); }
        let mut ids = BTreeSet::new();
        for rank in &self.ranks {
            if rank.book_id <= 0 || !ids.insert(rank.book_id) { return Err(RankingError::Invalid); }
        }
        if let Some(s) = &self.placement {
            let groups = tier_groups(&self.ranks);
            if s.placing_book_id <= 0 || ids.contains(&s.placing_book_id)
                || s.state_version == 0 || s.low_position < 0
                || s.high_position < s.low_position
                || s.high_position as usize >= groups.len()
            { return Err(RankingError::Invalid); }
        } else if !self.skipped_pairs.is_empty() {
            return Err(RankingError::Invalid);
        }
        Ok(())
    }

    pub fn sorted_ranks(&self) -> Vec<BookRank> {
        let mut result = self.ranks.clone();
        result.sort_by(|a,b| a.tier.cmp(&b.tier).then_with(|| a.book_id.to_string().cmp(&b.book_id.to_string())));
        result
    }

    pub fn next_question(&self) -> Result<Option<Question>, RankingError> {
        self.validate()?;
        let Some(s) = &self.placement else { return Ok(None); };
        let groups = tier_groups(&self.ranks);
        let mid = (s.low_position + s.high_position) / 2;
        let mut positions = (s.low_position..=s.high_position).collect::<Vec<_>>();
        positions.sort_by_key(|n| ((i64::from(*n) - i64::from(mid)).abs(), *n));
        for position in positions {
            for target_id in &groups[position as usize].1 {
                let (a,b) = canonical_pair(s.placing_book_id, *target_id)?;
                if self.skipped_pairs.contains(&(a,b)) { continue; }
                return Ok(Some(Question {
                    comparison_id: comparison_id(self.user_id,a,b,s.state_version),
                    book_a_id:a, book_b_id:b, placing_book_id:s.placing_book_id,
                    target_book_id:*target_id, state_version:s.state_version
                }));
            }
        }
        Ok(None)
    }

    /// Start only AFTER the caller has independently verified a trusted
    /// completed chapter belonging to this authenticated user.
    pub fn start_verified_placement(&self, book_id: i64)
        -> Result<(Self, AnswerStatus), RankingError>
    {
        self.validate()?;
        if book_id <= 0 || self.deferred_books.contains(&book_id) { return Err(RankingError::Invalid); }
        if self.ranks.iter().any(|rank| rank.book_id == book_id) {
            return Ok((self.clone(), AnswerStatus::AlreadyRanked));
        }
        if let Some(s) = &self.placement {
            if s.placing_book_id == book_id { return Ok((self.clone(), AnswerStatus::Placing)); }
            return Err(RankingError::Conflict);
        }
        if self.ranks.len() >= MAX_RANKED_BOOKS { return Err(RankingError::Invalid); }
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        if next.ranks.is_empty() {
            next.ranks.push(BookRank {book_id,tier:0,comparisons_count:0});
            return Ok((next,AnswerStatus::Ranked));
        }
        next.placement = Some(PlacementState {
            placing_book_id:book_id, low_position:0,
            high_position:i32::try_from(tier_groups(&next.ranks).len()).map_err(|_| RankingError::Invalid)?-1,
            state_version:next.revision,
        });
        next.skipped_pairs.clear();
        Ok((next,AnswerStatus::Placing))
    }

    /// A new *verified* reading event may release a previously deferred book.
    /// Callers must check event time > deferred_at in durable storage.
    pub fn release_deferred_after_verified_read(&self, book_id: i64) -> Result<Self, RankingError> {
        if book_id <= 0 { return Err(RankingError::Invalid); }
        let mut next = self.clone();
        next.deferred_books.remove(&book_id);
        Ok(next)
    }

    /// Skip exhaustion is sticky across refresh, until the user explicitly resumes.
    pub fn resume_exhausted_skips(&self) -> Result<Self, RankingError> {
        if self.placement.is_none() || self.next_question()?.is_some() || self.skipped_pairs.is_empty() {
            return Err(RankingError::Conflict);
        }
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        next.placement.as_mut().ok_or(RankingError::Invalid)?.state_version = next.revision;
        next.skipped_pairs.clear();
        Ok(next)
    }

    /// Compute a complete next snapshot; a PostgreSQL adapter must commit this
    /// with an atomic expected_state_version compare-and-swap.
    pub fn answer(&self, expected_state_version:u64, supplied_id:&str, outcome:Outcome)
        -> Result<(Self,AnswerStatus),RankingError>
    {
        self.validate()?;
        let state = self.placement.as_ref().ok_or(RankingError::Stale)?;
        if state.state_version != expected_state_version { return Err(RankingError::Stale); }
        let q = self.next_question()?.ok_or(RankingError::Exhausted)?;
        if q.comparison_id != supplied_id { return Err(RankingError::Stale); }
        let mut next = self.clone();
        if outcome == Outcome::Skip {
            next.skipped_pairs.insert(canonical_pair(q.book_a_id,q.book_b_id)?);
            return Ok((next,AnswerStatus::Skipped));
        }
        if matches!(outcome,Outcome::DeferA|Outcome::DeferB) {
            let id = if outcome==Outcome::DeferA { q.book_a_id } else { q.book_b_id };
            next.ranks.retain(|rank| rank.book_id != id);
            next.deferred_books.insert(id);
            next.placement = None;
            next.skipped_pairs.clear();
            next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
            return Ok((next,AnswerStatus::Deferred(id)));
        }
        let target_index = next.ranks.iter().position(|rank| rank.book_id == q.target_book_id)
            .ok_or(RankingError::Invalid)?;
        next.ranks[target_index].comparisons_count = next.ranks[target_index]
            .comparisons_count.checked_add(1).ok_or(RankingError::Invalid)?;
        next.revision = next.revision.checked_add(1).ok_or(RankingError::Invalid)?;
        if outcome == Outcome::Tie {
            let same_tier = next.ranks[target_index].tier;
            next.ranks.push(BookRank {
                book_id:q.placing_book_id,tier:same_tier,comparisons_count:state.state_version,
            });
            next.placement = None;
            next.skipped_pairs.clear();
            return Ok((next,AnswerStatus::Ranked));
        }
        let group_position = tier_groups(&self.ranks).iter()
            .position(|(_, ids)| ids.contains(&q.target_book_id)).ok_or(RankingError::Invalid)? as i32;
        let winner_id = if outcome == Outcome::AWins { q.book_a_id } else { q.book_b_id };
        let mut low = state.low_position;
        let mut high = state.high_position;
        if winner_id == state.placing_book_id { high = group_position - 1; }
        else { low = group_position + 1; }
        if low <= high {
            next.placement = Some(PlacementState {
                placing_book_id: state.placing_book_id,
                low_position: low, high_position: high, state_version:next.revision,
            });
            next.skipped_pairs.clear();
            return Ok((next,AnswerStatus::Placing));
        }
        let tier = tier_for_insert(&mut next.ranks, low as usize)?;
        next.ranks.push(BookRank {
            book_id:state.placing_book_id,tier,comparisons_count:state.state_version,
        });
        next.placement = None;
        next.skipped_pairs.clear();
        Ok((next,AnswerStatus::Ranked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed_two() -> RankingSnapshot {
        let (s,st)=RankingSnapshot::empty(7).unwrap().start_verified_placement(20).unwrap();
        assert_eq!(st, AnswerStatus::Ranked);
        let (s,st)=s.start_verified_placement(30).unwrap();
        assert_eq!(st, AnswerStatus::Placing);
        s
    }
    #[test]
    fn first_book_seeds_without_question_and_second_is_canonical() {
        let s=seed_two();
        let q=s.next_question().unwrap().unwrap();
        assert_eq!((q.book_a_id,q.book_b_id),(20,30));
        assert!(q.comparison_id.starts_with("book-compare-v1:"));
        assert_eq!(q.comparison_id,comparison_id(7,20,30,q.state_version));
    }
    #[test]
    fn winning_and_losing_ranks_do_not_mutate_the_original_snapshot() {
        let s=seed_two(); let q=s.next_question().unwrap().unwrap();
        let (ahead,status)=s.answer(q.state_version,&q.comparison_id,Outcome::BWins).unwrap();
        assert_eq!(status,AnswerStatus::Ranked);
        assert_eq!(ahead.sorted_ranks().iter().map(|r|r.book_id).collect::<Vec<_>>(),vec![30,20]);
        assert_eq!(ahead.ranks.iter().find(|r|r.book_id==20).unwrap().comparisons_count,1);
        assert_eq!(s.ranks.len(),1);
        let (behind,_) = s.answer(q.state_version,&q.comparison_id,Outcome::AWins).unwrap();
        assert_eq!(behind.sorted_ranks().iter().map(|r|r.book_id).collect::<Vec<_>>(),vec![20,30]);
        assert_eq!(ahead.ranks.iter().find(|r|r.book_id==30).unwrap().confidence(),0.5);
    }
    #[test]
    fn tie_group_preserves_shared_tier_and_deterministic_question() {
        let s=seed_two();let q=s.next_question().unwrap().unwrap();
        let (mut tied,status)=s.answer(q.state_version,&q.comparison_id,Outcome::Tie).unwrap();
        assert_eq!(status,AnswerStatus::Ranked);
        assert_eq!(tied.ranks[0].tier,tied.ranks[1].tier);
        let (s,st)=tied.start_verified_placement(5).unwrap();tied=s;
        assert_eq!(st,AnswerStatus::Placing);
        let q=tied.next_question().unwrap().unwrap();
        assert_eq!(q.target_book_id,20);
        assert_eq!(q.book_a_id,20); // string BookReference order: "20" < "5".
    }
    #[test]
    fn skip_is_sticky_until_explicit_resume_and_stale_answer_is_rejected() {
        let s=seed_two();let q=s.next_question().unwrap().unwrap();
        let (skipped,st)=s.answer(q.state_version,&q.comparison_id,Outcome::Skip).unwrap();
        assert_eq!(st,AnswerStatus::Skipped);
        assert!(skipped.next_question().unwrap().is_none());
        assert_eq!(skipped.answer(q.state_version,&q.comparison_id,Outcome::AWins),Err(RankingError::Exhausted));
        assert!(skipped.start_verified_placement(30).unwrap().0.next_question().unwrap().is_none());
        let resumed=skipped.resume_exhausted_skips().unwrap();
        let new_q=resumed.next_question().unwrap().unwrap();
        assert_ne!(new_q.comparison_id,q.comparison_id);
        assert_eq!(resumed.answer(q.state_version,&q.comparison_id,Outcome::AWins),Err(RankingError::Stale));
    }
    #[test]
    fn deferring_a_ranked_target_removes_it_without_fabricating_next_read() {
        let s=seed_two();let q=s.next_question().unwrap().unwrap();
        let (s,st)=s.answer(q.state_version,&q.comparison_id,Outcome::DeferA).unwrap();
        assert_eq!(st,AnswerStatus::Deferred(20));
        assert!(s.ranks.is_empty());
        assert_eq!(s.start_verified_placement(20),Err(RankingError::Invalid));
        assert!(s.release_deferred_after_verified_read(20).unwrap().deferred_books.is_empty());
        assert!(s.placement.is_none());
    }
    #[test]
    fn placement_requires_valid_distinct_ranks_and_single_active_candidate() {
        let s=seed_two();
        assert_eq!(s.start_verified_placement(40),Err(RankingError::Conflict));
        let mut broken=s.clone();broken.ranks.push(broken.ranks[0].clone());
        assert_eq!(broken.validate(),Err(RankingError::Invalid));
        assert_eq!(RankingSnapshot::empty(0),Err(RankingError::Invalid));
        assert_eq!(s.start_verified_placement(-1),Err(RankingError::Invalid));
    }
    #[test]
    fn median_binary_placement_handles_multiple_existing_tiers() {
        let mut s=RankingSnapshot::empty(7).unwrap();
        for (id,tier) in [(1,-2*TIER_STEP),(2,-TIER_STEP),(3,0),(4,TIER_STEP),(5,2*TIER_STEP)] {
            s.ranks.push(BookRank{book_id:id,tier,comparisons_count:0});
        }
        let (s,_) = s.start_verified_placement(9).unwrap();
        let q=s.next_question().unwrap().unwrap();
        assert_eq!(q.target_book_id,3);
        let (s,st)=s.answer(q.state_version,&q.comparison_id,Outcome::BWins).unwrap();
        assert_eq!(st,AnswerStatus::Placing);
        let q=s.next_question().unwrap().unwrap();
        assert_eq!(q.target_book_id,1); // positions 0..1: midpoint 0
    }
    #[test]
    fn sparse_tiers_rebalance_without_losing_ties() {
        let mut ranks=vec![
            BookRank{book_id:1,tier:0,comparisons_count:0},
            BookRank{book_id:2,tier:1,comparisons_count:0},
            BookRank{book_id:3,tier:1,comparisons_count:0},
        ];
        let new_tier=tier_for_insert(&mut ranks,1).unwrap();
        assert!(ranks[0].tier<new_tier && new_tier<ranks[1].tier);
        assert_eq!(ranks[1].tier,ranks[2].tier);
    }
    #[test]
    fn ranking_is_strictly_personal_no_global_book_popularity() {
        let s=seed_two();let mut other=s.clone();other.user_id=8;
        let first=s.next_question().unwrap().unwrap();
        let second=other.next_question().unwrap().unwrap();
        assert_ne!(first.comparison_id,second.comparison_id);
        assert_eq!(other.answer(first.state_version,&first.comparison_id,Outcome::AWins),Err(RankingError::Stale));
    }
}
