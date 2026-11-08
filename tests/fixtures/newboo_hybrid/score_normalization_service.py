from __future__ import annotations

from collections import defaultdict

from app.recommendations.schemas import (
    RecommendationCandidate,
    RecommendationCandidateSource,
    RecommendationScoreNormalizationDecision,
    RecommendationScoreNormalizationRejectionReason,
)

MIN_NORMALIZED_RECOMMENDATION_SCORE = 0
MAX_NORMALIZED_RECOMMENDATION_SCORE = 1000


def _reject_score_normalization(
    reason: RecommendationScoreNormalizationRejectionReason,
) -> RecommendationScoreNormalizationDecision:
    # Создаёт rejected decision для score normalization.
    return RecommendationScoreNormalizationDecision(
        accepted=False,
        candidates=(),
        rejection_reason=reason,
    )


def _bounded_normalized_score(value: int) -> int:
    # Ограничивает normalized score шкалой 0..1000.
    return max(
        MIN_NORMALIZED_RECOMMENDATION_SCORE,
        min(MAX_NORMALIZED_RECOMMENDATION_SCORE, int(value)),
    )


def _candidate_with_normalized_score(
    candidate: RecommendationCandidate,
    normalized_score: int,
) -> RecommendationCandidate:
    # Возвращает candidate с normalized score без потери исходного контекста.
    return RecommendationCandidate(
        book_id=candidate.book_id,
        source=candidate.source,
        reason=candidate.reason,
        raw_score=candidate.raw_score,
        normalized_score=_bounded_normalized_score(normalized_score),
        source_book_id=candidate.source_book_id,
        content_profile=candidate.content_profile,
    )


def normalize_score_against_max(*, raw_score: int, max_raw_score: int) -> int:
    # Нормализует raw score относительно max score внутри source.
    clean_raw_score = max(0, int(raw_score))
    clean_max_score = max(0, int(max_raw_score))
    if clean_raw_score <= 0 or clean_max_score <= 0:
        return 0
    return _bounded_normalized_score(
        clean_raw_score * MAX_NORMALIZED_RECOMMENDATION_SCORE // clean_max_score
    )


def normalize_recommendation_candidate_scores(
    candidates: tuple[RecommendationCandidate, ...],
) -> RecommendationScoreNormalizationDecision:
    # Нормализует raw scores кандидатов отдельно внутри каждого source.
    if not candidates:
        return _reject_score_normalization("empty_candidates")

    candidates_by_source: dict[RecommendationCandidateSource, list[RecommendationCandidate]] = defaultdict(list)
    for candidate in candidates:
        candidates_by_source[candidate.source].append(candidate)

    max_score_by_source = {
        source: max(max(0, int(candidate.raw_score)) for candidate in source_candidates)
        for source, source_candidates in candidates_by_source.items()
    }
    if any(max_score <= 0 for max_score in max_score_by_source.values()):
        return _reject_score_normalization("missing_source_scores")

    normalized_candidates: list[RecommendationCandidate] = []
    for candidate in candidates:
        max_raw_score = max_score_by_source[candidate.source]
        normalized_score = normalize_score_against_max(
            raw_score=candidate.raw_score,
            max_raw_score=max_raw_score,
        )
        normalized_candidates.append(
            _candidate_with_normalized_score(candidate, normalized_score)
        )

    return RecommendationScoreNormalizationDecision(
        accepted=True,
        candidates=tuple(normalized_candidates),
        rejection_reason=None,
    )


def recommendation_candidate_normalized_score(candidate: RecommendationCandidate) -> int:
    # Возвращает normalized_score candidate или 0, если score ещё не нормализован.
    if candidate.normalized_score is None:
        return 0
    return _bounded_normalized_score(candidate.normalized_score)