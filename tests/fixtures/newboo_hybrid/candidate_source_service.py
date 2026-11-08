from __future__ import annotations
from app.core.models.ids import PublicBookId

from app.recommendations.schemas import (
    RecommendationCandidate,
    RecommendationCandidateBatchDecision,
    RecommendationCandidateSource,
    RecommendationCandidateSourceDecision,
    RecommendationCandidateSourceLimits,
    RecommendationCandidateSourceRejectionReason,
    RecommendationReason,
)

DEFAULT_RECOMMENDATION_CANDIDATE_SOURCE_LIMITS = RecommendationCandidateSourceLimits()

_ALLOWED_SOURCE_REASON_PAIRS: frozenset[tuple[str, str]] = frozenset(
    {
        ("behavioral", "readers_also_read"),
        ("content", "similar_by_content"),
        ("popularity", "popular"),
        ("popularity", "trending"),
        ("popularity", "new_release"),
        ("popularity", "fallback_popular"),
        ("related_books", "manually_related"),
    }
)


def _reject_candidate_source(
    reason: RecommendationCandidateSourceRejectionReason,
) -> RecommendationCandidateSourceDecision:
    # Создаёт rejected decision для одного candidate.
    return RecommendationCandidateSourceDecision(
        accepted=False,
        candidate=None,
        rejection_reason=reason,
    )


def _clean_text(value: object) -> str:
    # Нормализует входное значение к stripped string.
    return str(value or "").strip()


def _is_allowed_source_reason_pair(
    source: RecommendationCandidateSource,
    reason: RecommendationReason,
) -> bool:
    # Проверяет, что source и reason образуют допустимый bounded contract.
    return (_clean_text(source), _clean_text(reason)) in _ALLOWED_SOURCE_REASON_PAIRS


def build_recommendation_candidate(
    *,
    book_id: PublicBookId,
    source: RecommendationCandidateSource,
    reason: RecommendationReason,
    raw_score: int,
    normalized_score: int | None = None,
    source_book_id: str | None = None,
) -> RecommendationCandidateSourceDecision:
    # Создаёт RecommendationCandidate с validation source/reason/score contract.
    clean_book_id = _clean_text(book_id)
    if not clean_book_id:
        return _reject_candidate_source("empty_book_id")

    clean_source = _clean_text(source)
    if clean_source not in {"behavioral", "content", "popularity", "related_books"}:
        return _reject_candidate_source("invalid_source")

    clean_reason = _clean_text(reason)
    if clean_reason not in {
        "manually_related",
        "readers_also_read",
        "similar_by_content",
        "popular",
        "trending",
        "new_release",
        "fallback_popular",
    }:
        return _reject_candidate_source("invalid_reason")

    if not _is_allowed_source_reason_pair(source, reason):
        return _reject_candidate_source("invalid_reason")

    clean_raw_score = int(raw_score)
    if clean_raw_score <= 0:
        return _reject_candidate_source("invalid_raw_score")

    clean_normalized_score = None
    if normalized_score is not None:
        clean_normalized_score = max(0, min(1000, int(normalized_score)))

    clean_source_book_id = _clean_text(source_book_id)
    return RecommendationCandidateSourceDecision(
        accepted=True,
        candidate=RecommendationCandidate(
            book_id=clean_book_id,
            source=source,
            reason=reason,
            raw_score=clean_raw_score,
            normalized_score=clean_normalized_score,
            source_book_id=clean_source_book_id or None,
        ),
        rejection_reason=None,
    )


def build_recommendation_candidate_batch(
    candidates: tuple[RecommendationCandidateSourceDecision, ...],
    limits: RecommendationCandidateSourceLimits = DEFAULT_RECOMMENDATION_CANDIDATE_SOURCE_LIMITS,
) -> RecommendationCandidateBatchDecision:
    # Собирает bounded batch accepted candidates из source decisions.
    if len(candidates) > limits.max_candidates_per_source:
        return RecommendationCandidateBatchDecision(
            accepted=False,
            candidates=(),
            rejected_count=len(candidates),
            rejection_reason="batch_too_large",
        )

    accepted_candidates: list[RecommendationCandidate] = []
    rejected_count = 0
    for decision in candidates:
        if decision.accepted and decision.candidate is not None:
            accepted_candidates.append(decision.candidate)
            continue
        rejected_count += 1

    return RecommendationCandidateBatchDecision(
        accepted=True,
        candidates=tuple(accepted_candidates),
        rejected_count=rejected_count,
        rejection_reason=None,
    )


def is_dedicated_related_book_candidate(candidate: RecommendationCandidate) -> bool:
    # Ручные связи собираются отдельным блоком и не участвуют в hybrid blend.
    return candidate.source == "related_books" or candidate.reason == "manually_related"


def is_content_discovery_candidate(candidate: RecommendationCandidate) -> bool:
    # Проверяет, что candidate можно использовать в similar_by_content discovery.
    return candidate.source == "content" and candidate.reason == "similar_by_content"
