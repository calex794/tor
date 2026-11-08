from __future__ import annotations
from app.core.type_coercion import int_from_unknown
from app.core.models.ids import PublicBookId

from collections import defaultdict


from app.recommendations.schemas import (
    RecommendationCandidate,
    RecommendationCandidateSource,
    RecommendationHybridScoredCandidate,
    RecommendationHybridScoringDecision,
    RecommendationHybridScoringLimits,
    RecommendationHybridScoringRejectionReason,
    RecommendationReason,
    RecommendationSourceWeights,
)
from app.recommendations.services.candidate_source_service import (
    is_dedicated_related_book_candidate,
)
from app.recommendations.services.score_normalization_service import (
    recommendation_candidate_normalized_score,
)


DEFAULT_RECOMMENDATION_HYBRID_SCORING_LIMITS = RecommendationHybridScoringLimits()
MAX_RECOMMENDATION_HYBRID_FINAL_SCORE = 1000
MAX_RECOMMENDATION_HYBRID_QUALITY_BOOST = 50

_SOURCE_ORDER: dict[RecommendationCandidateSource, int] = {
    "behavioral": 0,
    "content": 1,
    "popularity": 2,
    "related_books": 3,
}

_REASON_ORDER: dict[RecommendationReason, int] = {
    "manually_related": 0,
    "readers_also_read": 1,
    "similar_by_content": 2,
    "trending": 3,
    "popular": 4,
    "new_release": 5,
    "fallback_popular": 6,
}


def _reject_hybrid_scoring(
    *,
    reason: RecommendationHybridScoringRejectionReason,
    input_candidates_seen: int,
    dedicated_candidates_skipped: int = 0,
) -> RecommendationHybridScoringDecision:
    # Создаёт rejected hybrid scoring decision без partial candidates.
    return RecommendationHybridScoringDecision(
        accepted=False,
        candidates=(),
        rejected_reason=reason,
        input_candidates_seen=input_candidates_seen,
        dedicated_candidates_skipped=dedicated_candidates_skipped,
    )


def _validate_weights(weights: RecommendationSourceWeights) -> bool:
    # Проверяет invariant weights sum == 100 и non-negative components.
    components = (
        int(weights.behavioral),
        int(weights.content),
        int(weights.popularity),
    )
    return all(component >= 0 for component in components) and sum(components) == 100


def _source_weight_percent(
    source: RecommendationCandidateSource,
    weights: RecommendationSourceWeights,
) -> int:
    # Возвращает weight percent для source в hybrid blend.
    if source == "behavioral":
        return int(weights.behavioral)
    if source == "content":
        return int(weights.content)
    if source == "popularity":
        return int(weights.popularity)
    return 0


def _source_contribution(
    candidate: RecommendationCandidate,
    weights: RecommendationSourceWeights,
) -> int:
    # Считает contribution source candidate в final hybrid score.
    normalized_score = recommendation_candidate_normalized_score(candidate)
    source_weight = _source_weight_percent(candidate.source, weights)
    return normalized_score * source_weight // 100


def _bounded_final_score(value: object) -> int:
    # Ограничивает final hybrid score шкалой 0..1000.
    try:
        clean = int_from_unknown(value)
    except (TypeError, ValueError):
        return 0
    return max(0, min(MAX_RECOMMENDATION_HYBRID_FINAL_SCORE, clean))


def _quality_signal_score(
    signal: object,
) -> int:
    # Возвращает bounded trusted quality score из offline attribution signal.
    trust_bucket = str(
        getattr(signal, "trust_bucket", "normal") or "normal"
    ).strip()
    if trust_bucket in {"degraded", "reject"}:
        return 0

    try:
        clean = int(getattr(signal, "quality_score", 0))
    except (TypeError, ValueError):
        return 0

    try:
        multiplier = int(
            getattr(signal, "quality_multiplier_per_mille", 1000)
        )
    except (TypeError, ValueError):
        multiplier = 1000

    multiplier = max(0, min(1000, multiplier))
    trusted_score = max(
        0,
        min(MAX_RECOMMENDATION_HYBRID_FINAL_SCORE, clean),
    )
    return trusted_score * multiplier // 1000


def _quality_scores_by_book_id(
    quality_signals: tuple[object, ...],
) -> dict[PublicBookId, int]:
    # Собирает максимальный quality score для каждой книги.
    scores: dict[PublicBookId, int] = {}

    for signal in quality_signals:
        book_id = PublicBookId(str(getattr(signal, "book_id", "") or "").strip())
        if not book_id:
            continue

        scores[book_id] = max(
            scores.get(book_id, 0),
            _quality_signal_score(signal),
        )

    return scores


def _quality_boost_for_book_id(
    book_id: PublicBookId,
    *,
    quality_scores: dict[PublicBookId, int],
) -> int:
    # Считает один мягкий bounded quality boost для итогового score книги.
    quality_score = quality_scores.get(book_id, 0)
    if quality_score <= 0:
        return 0

    return min(
        MAX_RECOMMENDATION_HYBRID_QUALITY_BOOST,
        quality_score // 20,
    )


def _source_sort_key(
    source: RecommendationCandidateSource,
) -> tuple[int, str]:
    # Возвращает deterministic sort key для candidate source.
    return (
        _SOURCE_ORDER.get(source, len(_SOURCE_ORDER)),
        str(source),
    )


def _reason_sort_key(
    reason: RecommendationReason,
) -> tuple[int, str]:
    # Возвращает deterministic sort key для recommendation reason.
    return (
        _REASON_ORDER.get(reason, len(_REASON_ORDER)),
        str(reason),
    )


def _candidate_tie_break_key(
    candidate: RecommendationCandidate,
) -> tuple[int, int, int, int, str, str]:
    # Выбирает сильнейшего кандидата детерминированно при равном weighted contribution.
    try:
        raw_score = max(0, int(candidate.raw_score))
    except (TypeError, ValueError):
        raw_score = 0

    return (
        -recommendation_candidate_normalized_score(candidate),
        -raw_score,
        _SOURCE_ORDER.get(candidate.source, len(_SOURCE_ORDER)),
        _REASON_ORDER.get(candidate.reason, len(_REASON_ORDER)),
        str(candidate.source_book_id or ""),
        str(candidate.book_id or ""),
    )

def _candidate_is_better(
    *,
    candidate: RecommendationCandidate,
    contribution: int,
    current_candidate: RecommendationCandidate | None,
    current_contribution: int | None,
) -> bool:
    # Проверяет, должен ли candidate заменить текущего победителя.
    if current_candidate is None or current_contribution is None:
        return True

    if contribution != current_contribution:
        return contribution > current_contribution

    return _candidate_tie_break_key(candidate) < _candidate_tie_break_key(
        current_candidate
    )

def _source_winner_sort_key(
    item: tuple[RecommendationCandidate, int],
) -> tuple[int, int, int, int, int, str, str]:
    # Сортирует победителей sources по вкладу и силе исходного кандидата.
    candidate, contribution = item
    return (
        -int(contribution),
        *_candidate_tie_break_key(candidate),
    )


def _ordered_reasons(
    *,
    source_winners: tuple[tuple[RecommendationCandidate, int], ...],
    all_reasons: set[RecommendationReason],
) -> tuple[RecommendationReason, ...]:
    # Возвращает причины сначала в порядке победивших sources, затем остальные.
    result: list[RecommendationReason] = []
    seen: set[RecommendationReason] = set()

    for candidate, _contribution in source_winners:
        if candidate.reason in seen:
            continue
        seen.add(candidate.reason)
        result.append(candidate.reason)

    for reason in sorted(all_reasons, key=_reason_sort_key):
        if reason in seen:
            continue
        seen.add(reason)
        result.append(reason)

    return tuple(result)


def _scored_sort_key(
    candidate: RecommendationHybridScoredCandidate,
) -> tuple[int, str]:
    # Возвращает deterministic sort key для scored candidates.
    return (-candidate.final_score, candidate.book_id)


def _apply_popularity_cap(
    candidates: tuple[RecommendationHybridScoredCandidate, ...],
    *,
    max_popularity_results: int,
) -> tuple[RecommendationHybridScoredCandidate, ...]:
    # Ограничивает число candidates, где best_source=popularity.
    result: list[RecommendationHybridScoredCandidate] = []
    popularity_count = 0

    for candidate in candidates:
        if candidate.best_source == "popularity":
            if popularity_count >= max_popularity_results:
                continue
            popularity_count += 1

        result.append(candidate)

    return tuple(result)


def score_hybrid_candidates(
    candidates: tuple[RecommendationCandidate, ...],
    *,
    weights: RecommendationSourceWeights,
    limit: int = 20,
    limits: RecommendationHybridScoringLimits = (
        DEFAULT_RECOMMENDATION_HYBRID_SCORING_LIMITS
    ),
    quality_signals: tuple[
        object,
        ...,
    ] = (),
) -> RecommendationHybridScoringDecision:
    # Смешивает по одному сильнейшему кандидату каждой книги внутри каждого source.
    if not candidates:
        return _reject_hybrid_scoring(
            reason="empty_candidates",
            input_candidates_seen=0,
        )

    if len(candidates) > limits.max_candidates:
        return _reject_hybrid_scoring(
            reason="invalid_limit",
            input_candidates_seen=len(candidates),
        )

    if limit < 1 or limit > limits.max_results:
        return _reject_hybrid_scoring(
            reason="invalid_limit",
            input_candidates_seen=len(candidates),
        )

    if not _validate_weights(weights):
        return _reject_hybrid_scoring(
            reason="invalid_weights",
            input_candidates_seen=len(candidates),
        )

    quality_scores = _quality_scores_by_book_id(
        tuple(quality_signals or ())
    )

    best_candidate_by_book_source: dict[
        tuple[str, RecommendationCandidateSource],
        RecommendationCandidate,
    ] = {}
    best_contribution_by_book_source: dict[
        tuple[str, RecommendationCandidateSource],
        int,
    ] = {}
    reasons_by_book_id: dict[
        str,
        set[RecommendationReason],
    ] = defaultdict(set)

    dedicated_candidates_skipped = 0

    for candidate in candidates:
        if is_dedicated_related_book_candidate(candidate):
            dedicated_candidates_skipped += 1
            continue

        normalized_score = recommendation_candidate_normalized_score(candidate)
        if normalized_score <= 0:
            return _reject_hybrid_scoring(
                reason="missing_normalized_scores",
                input_candidates_seen=len(candidates),
                dedicated_candidates_skipped=dedicated_candidates_skipped,
            )

        contribution = _source_contribution(candidate, weights)
        if contribution <= 0:
            continue

        book_id = str(candidate.book_id or "").strip()
        if not book_id:
            continue

        source_key = (book_id, candidate.source)
        current_candidate = best_candidate_by_book_source.get(source_key)
        current_contribution = best_contribution_by_book_source.get(source_key)

        if _candidate_is_better(
            candidate=candidate,
            contribution=contribution,
            current_candidate=current_candidate,
            current_contribution=current_contribution,
        ):
            best_candidate_by_book_source[source_key] = candidate
            best_contribution_by_book_source[source_key] = contribution

        reasons_by_book_id[book_id].add(candidate.reason)

    score_by_book_id: dict[str, int] = defaultdict(int)
    source_winners_by_book_id: dict[
        str,
        list[tuple[RecommendationCandidate, int]],
    ] = defaultdict(list)

    for source_key, candidate in best_candidate_by_book_source.items():
        book_id, _source = source_key
        contribution = best_contribution_by_book_source[source_key]

        score_by_book_id[book_id] += contribution
        source_winners_by_book_id[book_id].append(
            (candidate, contribution)
        )

    scored_candidates: list[RecommendationHybridScoredCandidate] = []

    for book_id, source_score in score_by_book_id.items():
        source_winners = tuple(
            sorted(
                source_winners_by_book_id[book_id],
                key=_source_winner_sort_key,
            )
        )
        if not source_winners:
            continue

        best_candidate = source_winners[0][0]

        final_score = _bounded_final_score(
            source_score
            + _quality_boost_for_book_id(
                PublicBookId(book_id),
                quality_scores=quality_scores,
            )
        )

        scored_candidates.append(
            RecommendationHybridScoredCandidate(
                book_id=book_id,
                final_score=final_score,
                sources=tuple(
                    candidate.source
                    for candidate, _contribution in source_winners
                ),
                reasons=_ordered_reasons(
                    source_winners=source_winners,
                    all_reasons=reasons_by_book_id[book_id],
                ),
                best_source=best_candidate.source,
                best_reason=best_candidate.reason,
                rank=0,
            )
        )

    ranked = tuple(
        sorted(
            scored_candidates,
            key=_scored_sort_key,
        )
    )

    capped = _apply_popularity_cap(
        ranked,
        max_popularity_results=limits.max_popularity_results,
    )
    limited = capped[:limit]

    ranked_limited = tuple(
        RecommendationHybridScoredCandidate(
            book_id=candidate.book_id,
            final_score=candidate.final_score,
            sources=candidate.sources,
            reasons=candidate.reasons,
            best_source=candidate.best_source,
            best_reason=candidate.best_reason,
            rank=index,
        )
        for index, candidate in enumerate(limited, start=1)
    )

    return RecommendationHybridScoringDecision(
        accepted=True,
        candidates=ranked_limited,
        rejected_reason=None,
        input_candidates_seen=len(candidates),
        dedicated_candidates_skipped=dedicated_candidates_skipped,
    )
