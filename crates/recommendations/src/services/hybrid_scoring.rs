//! REC-07 pure recommendations_v1 scoring, ported from pinned NEwBOO.
//! This accepts offline candidates, never HTTP inputs or paid placements.
//! Serving must still apply live discoverability before exposing any metadata.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CANDIDATES: usize = 500;
pub const MAX_RESULTS: usize = 50;
pub const MAX_POPULARITY_RESULTS: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Source {
    Behavioral,
    Content,
    Popularity,
    RelatedBooks,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Reason {
    ManuallyRelated,
    ReadersAlsoRead,
    SimilarByContent,
    Trending,
    Popular,
    NewRelease,
    FallbackPopular,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// Legacy public identifier; v1 deliberately uses lexicographic ties.
    pub book_id: String,
    pub source: Source,
    pub reason: Reason,
    pub raw_score: i64,
    pub normalized_score: Option<i64>,
    pub source_book_id: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Weights {
    pub behavioral: i32,
    pub content: i32,
    pub popularity: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trust {
    Normal,
    Watch,
    Degraded,
    Reject,
}

#[derive(Clone, Debug)]
pub struct QualitySignal {
    pub book_id: String,
    pub quality_score: i64,
    pub multiplier_per_mille: i64,
    pub trust: Trust,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoredCandidate {
    pub book_id: String,
    pub final_score: i64,
    pub sources: Vec<Source>,
    pub reasons: Vec<Reason>,
    pub best_source: Source,
    pub best_reason: Reason,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    EmptyCandidates,
    InvalidLimit,
    InvalidWeights,
    MissingNormalizedScores,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    pub candidates: Vec<ScoredCandidate>,
    pub rejected_reason: Option<Rejection>,
    pub input_candidates_seen: usize,
    pub dedicated_candidates_skipped: usize,
}

impl Decision {
    fn reject(&mut self, reason: Rejection) {
        self.candidates.clear();
        self.rejected_reason = Some(reason);
    }
}

fn normalized(candidate: &Candidate) -> i64 {
    candidate.normalized_score.unwrap_or(0).clamp(0, 1000)
}

fn tie_key(candidate: &Candidate) -> (Reverse<i64>, Reverse<i64>, Source, Reason, &str, &str) {
    (
        Reverse(normalized(candidate)),
        Reverse(candidate.raw_score.max(0)),
        candidate.source,
        candidate.reason,
        candidate.source_book_id.as_deref().unwrap_or(""),
        &candidate.book_id,
    )
}

/// Bounded integer scoring with one winner per (book, source), no database I/O.
/// Quality is trusted offline attribution: score=50 adds 2, score=1000 adds 50.
/// Quality signals must already be bounded by the caller's offline window.
pub fn score_hybrid_candidates(
    candidates: &[Candidate],
    weights: Weights,
    limit: usize,
    quality_signals: &[QualitySignal],
) -> Decision {
    let mut decision = Decision {
        candidates: Vec::new(),
        rejected_reason: None,
        input_candidates_seen: candidates.len(),
        dedicated_candidates_skipped: 0,
    };
    let rejection = if candidates.is_empty() {
        Some(Rejection::EmptyCandidates)
    } else if candidates.len() > MAX_CANDIDATES || !(1..=MAX_RESULTS).contains(&limit) {
        Some(Rejection::InvalidLimit)
    } else if [weights.behavioral, weights.content, weights.popularity]
        .iter()
        .any(|w| *w < 0)
        || i64::from(weights.behavioral)
            + i64::from(weights.content)
            + i64::from(weights.popularity)
            != 100
    {
        Some(Rejection::InvalidWeights)
    } else {
        None
    };
    if let Some(reason) = rejection {
        decision.reject(reason);
        return decision;
    }

    let mut winners: BTreeMap<(&str, Source), (&Candidate, i64)> = BTreeMap::new();
    let mut reasons: BTreeMap<&str, BTreeSet<Reason>> = BTreeMap::new();
    for candidate in candidates {
        if candidate.source == Source::RelatedBooks || candidate.reason == Reason::ManuallyRelated {
            decision.dedicated_candidates_skipped += 1;
            continue;
        }
        let score = normalized(candidate);
        if score <= 0 {
            decision.reject(Rejection::MissingNormalizedScores);
            return decision;
        }
        let weight = match candidate.source {
            Source::Behavioral => weights.behavioral,
            Source::Content => weights.content,
            Source::Popularity => weights.popularity,
            Source::RelatedBooks => 0,
        };
        let contribution = score * i64::from(weight) / 100;
        let id = candidate.book_id.trim();
        if contribution <= 0 || id.is_empty() {
            continue;
        }
        let key = (id, candidate.source);
        let replace = winners.get(&key).is_none_or(|(current, current_score)| {
            contribution > *current_score
                || (contribution == *current_score && tie_key(candidate) < tie_key(current))
        });
        if replace {
            winners.insert(key, (candidate, contribution));
        }
        reasons.entry(id).or_default().insert(candidate.reason);
    }
    let mut by_book: BTreeMap<&str, Vec<(&Candidate, i64)>> = BTreeMap::new();
    for ((id, _), winner) in winners {
        by_book.entry(id).or_default().push(winner);
    }
    // Keep only quality for the bounded candidate IDs, never allocate an entry
    // per arbitrary signal subject. Duplicate signals use max, not sum.
    let mut quality: BTreeMap<&str, i64> = BTreeMap::new();
    for signal in quality_signals {
        let id = signal.book_id.trim();
        if !by_book.contains_key(id) || matches!(signal.trust, Trust::Degraded | Trust::Reject) {
            continue;
        }
        let score =
            signal.quality_score.clamp(0, 1000) * signal.multiplier_per_mille.clamp(0, 1000) / 1000;
        let current = quality.entry(id).or_default();
        *current = (*current).max(score);
    }
    for (id, mut sources) in by_book {
        sources.sort_by(|(a, av), (b, bv)| bv.cmp(av).then_with(|| tie_key(a).cmp(&tie_key(b))));
        let (best, _) = sources[0];
        let mut ordered_reasons = Vec::new();
        for reason in sources
            .iter()
            .map(|(c, _)| c.reason)
            .chain(reasons[id].iter().copied())
        {
            if !ordered_reasons.contains(&reason) {
                ordered_reasons.push(reason);
            }
        }
        let base: i64 = sources.iter().map(|(_, score)| score).sum();
        let boost = quality.get(id).copied().unwrap_or(0) / 20;
        decision.candidates.push(ScoredCandidate {
            book_id: id.to_owned(),
            final_score: (base + boost.min(50)).clamp(0, 1000),
            sources: sources.iter().map(|(c, _)| c.source).collect(),
            reasons: ordered_reasons,
            best_source: best.source,
            best_reason: best.reason,
            rank: 0,
        });
    }
    decision.candidates.sort_by(|a, b| {
        b.final_score
            .cmp(&a.final_score)
            .then_with(|| a.book_id.cmp(&b.book_id))
    });
    let mut popularity = 0;
    decision.candidates.retain(|candidate| {
        if candidate.best_source == Source::Popularity {
            popularity += 1;
            popularity <= MAX_POPULARITY_RESULTS
        } else {
            true
        }
    });
    decision.candidates.truncate(limit);
    for (index, candidate) in decision.candidates.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }
    decision
}
