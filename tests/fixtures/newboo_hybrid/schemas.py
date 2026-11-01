from __future__ import annotations
from dataclasses import dataclass, field
from datetime import datetime
from typing import Literal, TypeAlias
RecommendationBookId: TypeAlias = str
RecommendationUserSubjectId: TypeAlias = str
RecommendationSignalType = Literal[
    "book_opened",
    "reader_opened",
    "chapter_started",
    "chapter_finished",
    "book_finished",
    "library_added",
    "bookmark_added",
    "purchase_completed",
    "catalog_clicked",
    "search_clicked",
]
RecommendationSubjectType = Literal[
    "user",
    "guest",
    "anonymous_session",
]
RecommendationCandidateSource = Literal[
    "behavioral",
    "content",
    "popularity",
    "related_books",
]
RecommendationBlock = Literal[
    "related_books",
    "similar_by_content",
    "readers_also_read",
    "fallback_popular",
]
RecommendationScenario = Literal[
    "default",
    "new_user",
    "active_reader",
    "hot_session",
    "new_book",
    "sparse_graph",
]
RecommendationSurface = Literal[
    "home",
    "book_detail",
    "reader_end",
    "catalog",
    "library",
]
RecommendationReason = Literal[
    "manually_related",
    "readers_also_read",
    "similar_by_content",
    "popular",
    "trending",
    "new_release",
    "fallback_popular",
]
RecommendationSignalRejectionReason = Literal[
    "missing_context",
    "unknown_route",
    "route_not_allowed",
    "scanner_risk",
    "unsupported_surface",
    "unsupported_product_area",
    "behavior_path_rejected",
    "behavior_path_degraded",
]
RecommendationSignalWeightRejectionReason = Literal[
    "unsupported_event_type",
    "invalid_event_type",
]
TransitionConfidenceReason = Literal[
    "full_gap",
    "long_gap",
    "too_long_gap",
    "invalid_order",
    "same_book",
]
RecommendationSignalIngestionRejectionReason = Literal[
    "empty_event_id",
    "invalid_weight",
    "missing_subject",
    "batch_too_large",
]
RecommendationAggregationRejectionReason = Literal[
    "too_many_events",
    "too_many_subjects",
    "too_many_events_per_subject",
    "too_many_pair_keys",
]
RecommendationTopEdgesRejectionReason = Literal[
    "too_many_transition_counts",
    "invalid_top_n",
]
RecommendationContentSimilarityRejectionReason = Literal[
    "same_book",
    "different_language",
    "source_unavailable",
    "candidate_unavailable",
    "no_content_overlap",
]
RecommendationCandidateSourceRejectionReason = Literal[
    "empty_book_id",
    "invalid_raw_score",
    "invalid_source",
    "invalid_reason",
    "batch_too_large",
]
RecommendationScoreNormalizationRejectionReason = Literal[
    "empty_candidates",
    "missing_source_scores",
]
RecommendationScenarioDetectionReason = Literal[
    "new_user",
    "hot_session",
    "new_book",
    "sparse_graph",
    "active_reader",
    "default",
]
RecommendationHybridScoringRejectionReason = Literal[
    "empty_candidates",
    "missing_normalized_scores",
    "invalid_limit",
    "invalid_weights",
]
RecommendationAssemblyRejectionReason = Literal[
    "invalid_limit",
]
RecommendationRepositoryRejectionReason = Literal[
    "too_many_source_books",
    "invalid_limit",
    "too_many_metadata_ids",
]
RecommendationServiceRejectionReason = Literal[
    "invalid_recent_books_read",
    "invalid_edge_read",
    "invalid_metadata_read",
    "invalid_fallback_read",
]
RecommendationRuntimeRejectionReason = Literal[
    "rollout_rejected",
    "subject_resolution_rejected",
    "experiment_not_treatment",
    "active_generation_missing",
    "generation_timeout",
    "generation_circuit_open",
    "generation_bulkhead_full",
    "generation_unavailable",
    "query_budget_exceeded",
    "load_shedding",
    "runtime_timeout",
    "invalid_recent_books_read",
    "invalid_edge_read",
    "invalid_metadata_read",
    "invalid_fallback_read",
]
RecommendationAnalyticsEventType = Literal[
    "recommendation_impression",
    "recommendation_click",
    "recommendation_action",
]
RecommendationActionType = Literal[
    "book_opened",
    "chapter_started",
    "chapter_finished",
    "library_added",
    "bookmark_added",
    "purchase_completed",
]
RecommendationAttributionRejectionReason = Literal[
    "missing_token",
    "missing_book_id",
    "invalid_event_type",
    "invalid_action_type",
    "invalid_context",
    "invalid_token",
]
RecommendationHealthRejectionReason = Literal[
    "invalid_catalog_size",
    "too_many_target_impression_rows",
    "too_many_attribution_daily_rows",
    "low_non_empty_rate",
    "high_fallback_rate",
    "high_full_fallback_rate",
    "high_target_concentration",
    "low_click_rate",
    "low_action_rate",
    "partial_attribution_window",
]
RecommendationExperimentVariant = Literal[
    "disabled",
    "control",
    "treatment",
]
RecommendationExperimentRejectionReason = Literal[
    "missing_subject_key",
    "invalid_rollout",
    "invalid_holdout",
]
RecommendationSubjectResolverRejectionReason = Literal[
    "missing_subject",
    "too_many_linked_guests",
]
RecommendationRolloutRejectionReason = Literal[
    "globally_disabled",
    "surface_disabled",
    "invalid_algorithm_version",
]
RecommendationPopularitySourceRejectionReason = Literal[
    "empty_book_id",
    "invalid_score",
    "invalid_reason",
    "candidate_unavailable",
    "too_many_candidates",
    "no_candidates",
]
RecommendationContentCandidateSourceRejectionReason = Literal[
    "empty_book_id",
    "same_book",
    "different_language",
    "source_unavailable",
    "candidate_unavailable",
    "no_content_overlap",
    "too_many_candidates",
    "no_candidates",
]
RecommendationBehavioralCandidateSourceRejectionReason = Literal[
    "empty_source_book_id",
    "empty_target_book_id",
    "self_edge",
    "invalid_score",
    "too_many_edges",
    "no_candidates",
]
DEFAULT_RECOMMENDATION_ALGORITHM_VERSION = "recommendations_v1"
RECOMMENDATION_RECENT_ACTIVITY_DAYS = 30
RECOMMENDATION_SESSION_IDLE_SECONDS = 30 * 60
@dataclass(frozen=True, slots=True)
class RecommendationSubject:
    # Описывает bounded subject для recent context без хранения персональной ленты.
    subject_type: RecommendationSubjectType
    subject_key: str
@dataclass(frozen=True, slots=True)
class RecommendationSignal:
    # Хранит clean recommendation signal contract без DB/YDB-зависимостей.
    event_id: str
    event_type: RecommendationSignalType
    subject: RecommendationSubject
    source_book_id: str | None
    target_book_id: str | None
    occurred_at: datetime
    weight: int
    route_context: dict[str, object]
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
@dataclass(frozen=True, slots=True)
class RecommendationSignalHygieneDecision:
    # Хранит pure decision о пригодности события для recommendation pipeline.
    eligible: bool
    rejection_reason: RecommendationSignalRejectionReason | None
    route_context: dict[str, object]
@dataclass(frozen=True, slots=True)
class RecommendationSignalWeightDecision:
    # Хранит pure decision о весе recommendation signal.
    accepted: bool
    weight: int
    rejection_reason: RecommendationSignalWeightRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationTransitionEvent:
    # Хранит clean weighted событие subject для offline transition aggregation.
    event_id: str
    subject_key: str
    book_id: RecommendationBookId
    occurred_at: datetime
    weight: int
@dataclass(frozen=True, slots=True)
class TransitionConfidenceDecision:
    # Хранит pure decision о доверии к переходу между двумя книгами.
    accepted: bool
    multiplier_percent: int
    reason: TransitionConfidenceReason
    gap_seconds: int
@dataclass(frozen=True, slots=True)
class RecommendationSignalIngestionDecision:
    # Хранит результат приёма одного recommendation signal.
    accepted: bool
    stored: bool
    duplicate: bool
    rejection_reason: RecommendationSignalIngestionRejectionReason | None
    signal: RecommendationSignal | None
@dataclass(frozen=True, slots=True)
class RecommendationSignalBatchIngestionDecision:
    # Хранит результат bounded batch ingestion.
    accepted: bool
    stored_count: int
    duplicate_count: int
    rejected_count: int
    rejection_reason: RecommendationSignalIngestionRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationTransitionCount:
    # Хранит агрегированный вес перехода source_book_id -> target_book_id.
    source_book_id: str
    target_book_id: str
    weight: int
    transition_count: int
@dataclass(frozen=True, slots=True)
class RecommendationAggregationLimits:
    # Хранит hard limits для bounded behavioral aggregation.
    max_events_per_batch: int = 5_000
    max_subjects_per_batch: int = 1_000
    max_events_per_subject: int = 200
    max_pair_counter_keys: int = 50_000
@dataclass(frozen=True, slots=True)
class RecommendationAggregationDecision:
    # Хранит результат pure behavioral aggregation.
    accepted: bool
    transition_counts: tuple[RecommendationTransitionCount, ...]
    rejected_reason: RecommendationAggregationRejectionReason | None
    events_seen: int
    subjects_seen: int
    skipped_transition_count: int
@dataclass(frozen=True, slots=True)
class RecommendationBehavioralEdge:
    # Хранит prepared behavioral edge source_book_id -> target_book_id.
    source_book_id: str
    target_book_id: str
    raw_weight: int
    transition_count: int
    target_popularity: int
    score: int
    rank: int
@dataclass(frozen=True, slots=True)
class RecommendationTopEdgesLimits:
    # Хранит hard limits для построения bounded top behavioral edges.
    max_transition_counts: int = 50_000
    max_top_n_per_source: int = 100
@dataclass(frozen=True, slots=True)
class RecommendationTopEdgesDecision:
    # Хранит результат pure построения top behavioral edges.
    accepted: bool
    edges: tuple[RecommendationBehavioralEdge, ...]
    rejected_reason: RecommendationTopEdgesRejectionReason | None
    transition_counts_seen: int
    source_books_seen: int
@dataclass(frozen=True, slots=True)
class RecommendationBookContentProfile:
    # Хранит bounded metadata книги для pure content similarity.
    book_id: RecommendationBookId
    language: str
    author_id: str | None = None
    tag_ids: tuple[str, ...] = field(default_factory=tuple)
    genre_ids: tuple[str, ...] = field(default_factory=tuple)
    is_available: bool = True
@dataclass(frozen=True, slots=True)
class RecommendationContentSimilarityDecision:
    # Хранит pure decision о content similarity между двумя книгами.
    accepted: bool
    score: int
    rejection_reason: RecommendationContentSimilarityRejectionReason | None
    shared_tag_count: int
    shared_genre_count: int
    same_author: bool
    language_match: bool = False
    tag_score: int = 0
    genre_score: int = 0
    author_score: int = 0
    language_score: int = 0
@dataclass(frozen=True, slots=True)
class RecommendationCandidateSourceLimits:
    # Хранит hard limits для bounded candidate source conversion.
    max_candidates_per_source: int = 100
@dataclass(frozen=True, slots=True)
class RecommendationCandidateSourceDecision:
    # Хранит результат подготовки одного recommendation candidate.
    accepted: bool
    candidate: RecommendationCandidate | None
    rejection_reason: RecommendationCandidateSourceRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationCandidateBatchDecision:
    # Хранит результат bounded подготовки candidates одного source.
    accepted: bool
    candidates: tuple[RecommendationCandidate, ...]
    rejected_count: int
    rejection_reason: RecommendationCandidateSourceRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationScoreNormalizationDecision:
    # Хранит результат нормализации candidates одного source.
    accepted: bool
    candidates: tuple[RecommendationCandidate, ...]
    rejection_reason: RecommendationScoreNormalizationRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationScenarioFacts:
    # Хранит bounded facts для выбора scenario без raw identity.
    recent_books_count: int = 0
    recent_signal_count: int = 0
    behavioral_candidates_count: int = 0
    content_candidates_count: int = 0
    source_book_age_days: int | None = None
    source_book_behavioral_edge_count: int = 0
    session_event_count: int = 0
@dataclass(frozen=True, slots=True)
class RecommendationSourceWeights:
    # Хранит source weights для adaptive hybrid scoring.
    behavioral: int
    content: int
    popularity: int
@dataclass(frozen=True, slots=True)
class RecommendationScenarioDecision:
    # Хранит выбранный recommendation scenario и его weights.
    scenario: RecommendationScenario
    reason: RecommendationScenarioDetectionReason
    weights: RecommendationSourceWeights
@dataclass(frozen=True, slots=True)
class RecommendationHybridScoredCandidate:
    # Хранит candidate после adaptive hybrid scoring.
    book_id: RecommendationBookId
    final_score: int
    sources: tuple[RecommendationCandidateSource, ...]
    reasons: tuple[RecommendationReason, ...]
    best_source: RecommendationCandidateSource
    best_reason: RecommendationReason
    rank: int
@dataclass(frozen=True, slots=True)
class RecommendationHybridScoringLimits:
    # Хранит hard limits для bounded hybrid scoring.
    max_candidates: int = 500
    max_results: int = 50
    max_popularity_results: int = 10
@dataclass(frozen=True, slots=True)
class RecommendationHybridScoringDecision:
    # Хранит результат pure adaptive hybrid scoring.
    accepted: bool
    candidates: tuple[RecommendationHybridScoredCandidate, ...]
    rejected_reason: RecommendationHybridScoringRejectionReason | None
    input_candidates_seen: int
    dedicated_candidates_skipped: int
@dataclass(frozen=True, slots=True)
class RecommendationAssemblyBookMetadata:
    # Хранит bounded metadata для runtime filters, diversity и scenario facts.
    book_id: RecommendationBookId
    author_id: str | None = None
    is_available: bool = True
    created_at: datetime | None = None
@dataclass(frozen=True, slots=True)
class RecommendationBookCoverProjection:
    # Хранит bounded cover choice без полного book row.
    url: str
    cover_slot: int | None = None
    cover_id: str = ""
@dataclass(frozen=True, slots=True)
class RecommendationBookCardProjection:
    # Хранит только данные, необходимые recommendation card view-model.
    book_id: RecommendationBookId
    route_ref: str
    title: str
    description: str = ""
    author_name: str = ""
    covers: tuple[RecommendationBookCoverProjection, ...] = field(default_factory=tuple)
    genres: tuple[str, ...] = field(default_factory=tuple)
    tags: tuple[str, ...] = field(default_factory=tuple)
    fandoms: tuple[str, ...] = field(default_factory=tuple)
@dataclass(frozen=True, slots=True)
class RecommendationLoadedBookData:
    # Объединяет две immutable projection, построенные из одной версии book row.
    metadata: RecommendationAssemblyBookMetadata
    card: RecommendationBookCardProjection | None
@dataclass(frozen=True, slots=True)
class RecommendationAssemblyLimits:
    # Хранит hard/soft limits для bounded runtime assembly.
    max_items: int = 50
    max_related_books_items: int = 10
    max_fallback_items: int = 20
    max_per_author: int = 2
@dataclass(frozen=True, slots=True)
class RecommendationAssemblyDecision:
    # Хранит результат pure runtime assembly.
    accepted: bool
    response: RecommendationResponse | None
    rejected_reason: RecommendationAssemblyRejectionReason | None
    input_items_seen: int
    excluded_count: int
    deduped_count: int
    unavailable_count: int
    fallback_used: bool
    diversity_excluded_count: int = 0
@dataclass(frozen=True, slots=True)
class RecommendationRepositoryLimits:
    # Хранит hard limits repository contracts для serving path.
    max_recent_books: int = 5
    max_source_books: int = 5
    max_edges_per_source: int = 100
    max_metadata_books: int = 200
    max_fallback_books: int = 50
@dataclass(frozen=True, slots=True)
class RecommendationRecentBook:
    # Хранит recent book item для bounded recent context.
    book_id: RecommendationBookId
    occurred_at: datetime
    weight: int
@dataclass(frozen=True, slots=True)
class RecommendationSubjectActivityContext:
    # Хранит prepared activity facts для scenario detection без raw event scan.
    recent_signal_count: int = 0
    session_event_count: int = 0
    last_activity_at: datetime | None = None
@dataclass(frozen=True, slots=True)
class RecommendationRepositoryReadDecision:
    # Хранит generic result repository read с bounded rejection reason.
    accepted: bool
    rejection_reason: RecommendationRepositoryRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationServiceDecision:
    # Хранит результат service facade без route/UI side effects.
    accepted: bool
    response: RecommendationResponse
    rejection_reason: RecommendationServiceRejectionReason | None
    degraded: bool
    recent_books_count: int
    behavioral_edges_count: int
    metadata_books_count: int
    fallback_candidates_count: int
@dataclass(frozen=True, slots=True)
class RecommendationRuntimeDecision:
    # Хранит результат gated runtime facade без route/UI dependencies.
    accepted: bool
    response: RecommendationResponse
    rejection_reason: RecommendationRuntimeRejectionReason | None
    rollout_accepted: bool
    experiment_variant: RecommendationExperimentVariant
    subject_keys_count: int
@dataclass(frozen=True, slots=True)
class RecommendationAttributionContext:
    # Хранит bounded attribution context для recommendation analytics events.
    rec_token: str
    book_id: RecommendationBookId
    block: RecommendationBlock
    source: RecommendationCandidateSource
    reason: RecommendationReason
    surface: RecommendationSurface
    scenario: RecommendationScenario
    position: int
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
    generation_id: str = "unknown-generation"
    subject_binding: str = ""
    accepted_subject_bindings: tuple[str, ...] = field(
        default_factory=tuple
    )
    cover_slot: int | None = None
    cover_id: str | None = None
@dataclass(frozen=True, slots=True)
class RecommendationAnalyticsEvent:
    # Хранит analytics event payload без high-cardinality metric labels.
    event_type: RecommendationAnalyticsEventType
    attribution: RecommendationAttributionContext
    action_type: RecommendationActionType | None = None
@dataclass(frozen=True, slots=True)
class RecommendationAttributionDecision:
    # Хранит результат построения bounded attribution event.
    accepted: bool
    event: RecommendationAnalyticsEvent | None
    rejection_reason: RecommendationAttributionRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationMetricLabels:
    # Хранит low-cardinality labels для recommendation metrics.
    event_type: RecommendationAnalyticsEventType
    block: RecommendationBlock
    source: RecommendationCandidateSource
    surface: RecommendationSurface
    scenario: RecommendationScenario
    algorithm_version: str
@dataclass(frozen=True, slots=True)
class RecommendationTargetImpressionCount:
    # Хранит aggregate count показов target book без использования book_id как metric label.
    book_id: RecommendationBookId
    impression_count: int
@dataclass(frozen=True, slots=True)
class RecommendationHealthSnapshot:
    # Хранит bounded aggregate snapshot для health metrics рекомендаций.
    eligible_catalog_books: int
    eligible_recommendation_requests: int
    recommendation_responses: int
    non_empty_recommendation_responses: int
    fallback_responses: int
    full_fallback_responses: int
    books_with_behavioral_edges: int
    books_with_content_edges: int
    books_with_related_books: int
    unique_recommended_books: int
    attribution_impressions: int = 0
    attribution_clicks: int = 0
    attribution_actions: int = 0
    attribution_purchases: int = 0
    target_impression_counts: tuple[RecommendationTargetImpressionCount, ...] = field(default_factory=tuple)
@dataclass(frozen=True, slots=True)
class RecommendationHealthLimits:
    # Хранит hard limits для bounded health aggregate input.
    max_target_impression_rows: int = 1_000
    max_attribution_daily_rows: int = 5_000
@dataclass(frozen=True, slots=True)
class RecommendationHealthMetrics:
    # Хранит recommendation health metrics в integer per-mille scale.
    behavioral_edge_coverage_per_mille: int
    content_edge_coverage_per_mille: int
    related_books_coverage_per_mille: int
    serving_coverage_per_mille: int
    fallback_rate_per_mille: int
    full_fallback_rate_per_mille: int
    catalog_diversity_per_mille: int
    top_10_target_share_per_mille: int
    top_100_target_share_per_mille: int
    attribution_click_rate_per_mille: int
    attribution_action_rate_per_mille: int
    attribution_purchase_rate_per_mille: int
@dataclass(frozen=True, slots=True)
class RecommendationHealthDecision:
    # Хранит результат pure расчёта health metrics.
    accepted: bool
    metrics: RecommendationHealthMetrics | None
    rejection_reason: RecommendationHealthRejectionReason | None
    target_impression_rows_seen: int
@dataclass(frozen=True, slots=True)
class RecommendationExperimentConfig:
    # Хранит bounded experiment config для recommendations rollout.
    enabled: bool
    experiment_name: str
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
    rollout_percent: int = 100
    holdout_percent: int = 10
@dataclass(frozen=True, slots=True)
class RecommendationExperimentDecision:
    # Хранит deterministic assignment в recommendation experiment.
    accepted: bool
    variant: RecommendationExperimentVariant
    experiment_name: str
    algorithm_version: str
    rejection_reason: RecommendationExperimentRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationSubjectResolverConfig:
    # Хранит hard limits для subject resolver.
    max_linked_guest_sessions: int = 5
@dataclass(frozen=True, slots=True)
class RecommendationSubjectResolverInput:
    # Хранит входные identity hints без raw request/cookie payload.
    user_id: RecommendationUserSubjectId | None = None
    guest_session_id: str | None = None
    linked_guest_session_ids: tuple[str, ...] = field(default_factory=tuple)
@dataclass(frozen=True, slots=True)
class RecommendationSubjectResolverDecision:
    # Хранит bounded subject_keys для recent recommendation context.
    accepted: bool
    subject_keys: tuple[str, ...]
    rejection_reason: RecommendationSubjectResolverRejectionReason | None
    linked_guest_sessions_used: int
@dataclass(frozen=True, slots=True)
class RecommendationSurfaceRolloutConfig:
    # Хранит per-surface rollout flags для recommendations.
    home_enabled: bool = False
    book_detail_enabled: bool = False
    reader_end_enabled: bool = False
    catalog_enabled: bool = False
    library_enabled: bool = False
@dataclass(frozen=True, slots=True)
class RecommendationRolloutConfig:
    # Хранит bounded rollout config без прямого чтения settings/env.
    enabled: bool
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
    degraded_fallback_enabled: bool = True
    surfaces: RecommendationSurfaceRolloutConfig = field(
        default_factory=RecommendationSurfaceRolloutConfig
    )
@dataclass(frozen=True, slots=True)
class RecommendationRolloutDecision:
    # Хранит решение rollout gate для конкретной recommendation surface.
    accepted: bool
    surface: RecommendationSurface
    algorithm_version: str
    degraded_fallback_enabled: bool
    rejection_reason: RecommendationRolloutRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationPopularityCandidateRow:
    # Хранит prepared popularity/fallback row без raw event scan.
    book_id: RecommendationBookId
    score: int
    reason: RecommendationReason
    is_available: bool = True
@dataclass(frozen=True, slots=True)
class RecommendationPopularitySourceLimits:
    # Хранит hard limits для bounded popularity source.
    max_candidate_rows: int = 100
    max_output_candidates: int = 50
@dataclass(frozen=True, slots=True)
class RecommendationPopularitySourceDecision:
    # Хранит результат построения popularity/fallback candidates.
    accepted: bool
    candidates: tuple[RecommendationCandidate, ...]
    rejection_reason: RecommendationPopularitySourceRejectionReason | None
    rows_seen: int
    rejected_count: int
@dataclass(frozen=True, slots=True)
class RecommendationContentCandidateSourceLimits:
    # Хранит hard limits для bounded content candidate source.
    max_candidate_profiles: int = 100
    max_output_candidates: int = 50
@dataclass(frozen=True, slots=True)
class RecommendationContentCandidateDecision:
    # Хранит результат построения одного content recommendation candidate.
    accepted: bool
    candidate: RecommendationCandidate | None
    rejection_reason: RecommendationContentCandidateSourceRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationContentCandidateBatchDecision:
    # Хранит результат bounded построения content candidates.
    accepted: bool
    candidates: tuple[RecommendationCandidate, ...]
    rejection_reason: RecommendationContentCandidateSourceRejectionReason | None
    profiles_seen: int
    rejected_count: int
@dataclass(frozen=True, slots=True)
class RecommendationBehavioralCandidateSourceLimits:
    # Хранит hard limits для bounded behavioral candidate source.
    max_edge_rows: int = 100
    max_output_candidates: int = 50
@dataclass(frozen=True, slots=True)
class RecommendationBehavioralCandidateDecision:
    # Хранит результат построения одного behavioral recommendation candidate.
    accepted: bool
    candidate: RecommendationCandidate | None
    rejection_reason: RecommendationBehavioralCandidateSourceRejectionReason | None
@dataclass(frozen=True, slots=True)
class RecommendationBehavioralCandidateBatchDecision:
    # Хранит результат bounded построения behavioral candidates.
    accepted: bool
    candidates: tuple[RecommendationCandidate, ...]
    rejection_reason: RecommendationBehavioralCandidateSourceRejectionReason | None
    edges_seen: int
    rejected_count: int
@dataclass(frozen=True, slots=True)
class RecommendationCandidate:
    # Описывает кандидата из одного source до финальной hybrid assembly.
    book_id: RecommendationBookId
    source: RecommendationCandidateSource
    reason: RecommendationReason
    raw_score: int
    normalized_score: int | None = None
    source_book_id: str | None = None
    content_profile: RecommendationBookContentProfile | None = None
@dataclass(frozen=True, slots=True)
class RecommendationItem:
    # Описывает финальный item после scoring/dedupe/diversity.
    book_id: RecommendationBookId
    block: RecommendationBlock
    source: RecommendationCandidateSource
    reason: RecommendationReason
    final_score: int
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
    social_display_reason_kind: str = ""
    social_display_reason: str = ""
    social_actor_count: int = 0
    card: RecommendationBookCardProjection | None = None
@dataclass(frozen=True, slots=True)
class RecommendationRequest:
    # Описывает bounded request на сборку рекомендаций без raw identity.
    surface: RecommendationSurface
    scenario: RecommendationScenario
    limit: int
    current_book_id: str | None = None
    source_book_ids: tuple[str, ...] = field(default_factory=tuple)
    exclude_book_ids: tuple[str, ...] = field(default_factory=tuple)
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
@dataclass(frozen=True, slots=True)
class RecommendationResolvedGeneration:
    # Immutable consistency boundary resolved once for one serving request.
    surface: RecommendationSurface
    generation_id: str
    algorithm_version: str
    activated_at: datetime | None = None
@dataclass(frozen=True, slots=True)
class RecommendationResponse:
    # Описывает bounded response рекомендаций с флагами деградации.
    items: tuple[RecommendationItem, ...]
    surface: RecommendationSurface
    scenario: RecommendationScenario
    degraded: bool
    fallback_used: bool
    algorithm_version: str = DEFAULT_RECOMMENDATION_ALGORITHM_VERSION
    generation_id: str = "unknown-generation"
    degradation_reasons: tuple[str, ...] = field(default_factory=tuple)
