//! REC-10: last-mile public metadata assembly for already scored candidates.
//! The PostgreSQL adapter must recheck live book visibility on EVERY read.
//! This deliberately does not implement the still-missing A/B age/access policy
//! or expose an HTTP endpoint. A web adapter must add those gates and HTML escaping.
use std::collections::{BTreeMap, BTreeSet};

use super::hybrid_scoring::{Reason, ScoredCandidate, Source, MAX_RESULTS};

pub const MAX_RESPONSE_ITEMS: usize = 20;
pub const ALGORITHM_VERSION: &str = "recommendations_v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblyError {
    Invalid,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicRecommendation {
    pub book_id: i64,
    pub title: String,
    pub author_name: String,
    pub language: String,
    pub source: Source,
    pub reason: Reason,
    pub rank: usize,
    pub algorithm_version: &'static str,
}

/// Only canonical public book metadata. No chapter body, entitlements, paid
/// URLs, private preferences, user identifiers or raw behavioral signals.
#[derive(Clone, Debug)]
pub(crate) struct VisibleBookMetadata {
    pub book_id: i64,
    pub title: String,
    pub author_name: String,
    pub language: String,
}

/// Validate the scorer's bounded ordered IDs before any PostgreSQL lookup.
/// Rejecting rather than skipping malformed IDs prevents partial, ambiguous
/// candidate lists from being mixed with unrelated canonical book identities.
pub(crate) fn requested_book_ids(
    scored: &[ScoredCandidate],
    limit: usize,
    exclude_book_id: Option<i64>,
) -> Result<Vec<i64>, AssemblyError> {
    if !(1..=MAX_RESPONSE_ITEMS).contains(&limit)
        || scored.len() > MAX_RESULTS
        || exclude_book_id.is_some_and(|id| id <= 0)
    {
        return Err(AssemblyError::Invalid);
    }
    let mut seen = BTreeSet::new();
    let mut ids = Vec::with_capacity(scored.len());
    for (index, candidate) in scored.iter().enumerate() {
        let id = candidate
            .book_id
            .parse::<i64>()
            .map_err(|_| AssemblyError::Invalid)?;
        if id <= 0
            || candidate.book_id != id.to_string()
            || candidate.rank != index + 1
            || !seen.insert(id)
        {
            return Err(AssemblyError::Invalid);
        }
        if Some(id) != exclude_book_id {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Assemble only from the adapter's just-fetched, already visibility-checked
/// rows. Missing books are omitted WITHOUT filling gaps from unchecked caches.
pub(crate) fn assemble_visible(
    scored: &[ScoredCandidate],
    rows: Vec<VisibleBookMetadata>,
    limit: usize,
    exclude_book_id: Option<i64>,
) -> Result<Vec<PublicRecommendation>, AssemblyError> {
    let requested = requested_book_ids(scored, limit, exclude_book_id)?;
    if rows.len() > requested.len() {
        return Err(AssemblyError::Invalid);
    }
    let requested: BTreeSet<i64> = requested.into_iter().collect();
    let mut by_id = BTreeMap::new();
    for book in rows {
        if !requested.contains(&book.book_id)
            || book.title.trim().is_empty()
            || book.language.is_empty()
            || by_id.insert(book.book_id, book.clone()).is_some()
        {
            return Err(AssemblyError::Invalid);
        }
    }
    let mut items = Vec::with_capacity(limit);
    for candidate in scored {
        let id = candidate
            .book_id
            .parse::<i64>()
            .map_err(|_| AssemblyError::Invalid)?;
        if Some(id) == exclude_book_id {
            continue;
        }
        if let Some(book) = by_id.remove(&id) {
            items.push(PublicRecommendation {
                book_id: id,
                title: book.title,
                author_name: book.author_name,
                language: book.language,
                source: candidate.best_source,
                reason: candidate.best_reason,
                rank: items.len() + 1,
                algorithm_version: ALGORITHM_VERSION,
            });
            if items.len() == limit {
                break;
            }
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(id: &str, rank: usize) -> ScoredCandidate {
        ScoredCandidate {
            book_id: id.to_owned(),
            final_score: 100 - rank as i64,
            sources: vec![Source::Content],
            reasons: vec![Reason::SimilarByContent],
            best_source: Source::Content,
            best_reason: Reason::SimilarByContent,
            rank,
        }
    }

    fn metadata(id: i64) -> VisibleBookMetadata {
        VisibleBookMetadata {
            book_id: id,
            title: format!("Book {id}"),
            author_name: "Public author".to_owned(),
            language: "ru".to_owned(),
        }
    }

    #[test]
    fn rec_10_preserves_scoring_order_but_rechecks_only_visible_metadata() {
        let input = vec![scored("4", 1), scored("2", 2), scored("3", 3)];
        let result = assemble_visible(&input, vec![metadata(3), metadata(4)], 20, None).unwrap();
        assert_eq!(
            result.iter().map(|b| b.book_id).collect::<Vec<_>>(),
            vec![4, 3]
        );
        assert_eq!(result[0].rank, 1);
        assert_eq!(result[1].rank, 2);
        assert_eq!(result[0].source, Source::Content);
        assert_eq!(result[0].reason, Reason::SimilarByContent);
        assert_eq!(result[0].algorithm_version, "recommendations_v1");
        assert_eq!(assemble_visible(&input, vec![], 20, None).unwrap(), []);
    }

    #[test]
    fn rec_10_fails_closed_on_malformed_or_duplicate_scored_ids() {
        for id in ["0", "-4", "04", "a", "1;drop", " 4"] {
            assert_eq!(
                requested_book_ids(&[scored(id, 1)], 20, None),
                Err(AssemblyError::Invalid),
                "{id}"
            );
        }
        assert_eq!(
            requested_book_ids(&[scored("2", 1), scored("2", 2)], 20, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            requested_book_ids(&[scored("2", 2)], 20, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            requested_book_ids(&[scored("2", 1)], 0, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            requested_book_ids(&[scored("2", 1)], 21, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            requested_book_ids(&[scored("2", 1)], 20, Some(0)),
            Err(AssemblyError::Invalid)
        );
    }

    #[test]
    fn rec_10_rejects_extra_or_duplicated_metadata_instead_of_leaking_it() {
        let input = vec![scored("2", 1)];
        assert_eq!(
            assemble_visible(&input, vec![metadata(99)], 20, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            assemble_visible(&input, vec![metadata(2), metadata(2)], 20, None),
            Err(AssemblyError::Invalid)
        );
        assert_eq!(
            assemble_visible(&input, vec![metadata(2)], 20, Some(2)).unwrap(),
            []
        );
    }

    #[test]
    fn rec_10_bounded_response_does_not_backfill_unauthorized_ids() {
        let input = vec![scored("2", 1), scored("3", 2), scored("4", 3)];
        let result = assemble_visible(&input, vec![metadata(4), metadata(2)], 1, None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].book_id, 2);
    }
}
