use tor_contracts::flow::{
    count_canonical_conversions, AcquisitionChannel, Channels, DiscoveryChannel, FactAuthority,
    FlowContractError, Metric, PageViewEvidence, PeriodAggregation, RequestMethod, ResponseKind,
    RouteSurface, TrafficHygiene, VerifiedConversion, VerifiedFact, FLOW_CONTRACT_VERSION,
    MAX_CONVERSION_RECONCILIATION_ROWS, VISIT_IDLE_TIMEOUT_SECONDS,
};

const CHANNELS: Channels = Channels {
    acquisition: AcquisitionChannel::ExternalOrganicSearch,
    discovery: DiscoveryChannel::OrganicRecommendation,
};

fn page(surface: RouteSurface) -> PageViewEvidence {
    PageViewEvidence {
        method: RequestMethod::Get,
        status: 200,
        response_kind: ResponseKind::Html,
        surface,
        hygiene: TrafficHygiene::Eligible,
        visibility_confirmed: true,
    }
}

fn finish(id: u8, channels: Channels) -> VerifiedConversion {
    VerifiedConversion::new(
        [id; 16],
        VerifiedFact::ChapterFinished,
        FactAuthority::ReaderTransaction,
        channels,
    )
    .expect("trusted chapter completion")
}

#[test]
fn flow_01_only_successful_verified_html_pages_enter_funnel() {
    assert_eq!(FLOW_CONTRACT_VERSION, "flow_v1");
    assert_eq!(VISIT_IDLE_TIMEOUT_SECONDS, 1800);
    for route in [
        RouteSurface::Home,
        RouteSurface::Catalogue,
        RouteSurface::BookDetail,
        RouteSurface::ChapterReader,
        RouteSurface::AuthenticatedPage,
    ] {
        assert!(page(route).eligible_for_funnel(), "{route:?}");
    }
    for route in [
        RouteSurface::StaticAsset,
        RouteSurface::Api,
        RouteSurface::Ops,
        RouteSurface::Unknown,
    ] {
        assert!(!page(route).eligible_for_funnel(), "{route:?}");
    }
    assert!(!PageViewEvidence {
        method: RequestMethod::Other,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        status: 404,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        status: 302,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        status: 500,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        response_kind: ResponseKind::Other,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        visibility_confirmed: false,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        hygiene: TrafficHygiene::SuspectedAutomation,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
    assert!(!PageViewEvidence {
        hygiene: TrafficHygiene::Unclassified,
        ..page(RouteSurface::BookDetail)
    }
    .eligible_for_funnel());
}

#[test]
fn flow_01_separate_external_acquisition_from_internal_discovery() {
    let event = finish(1, CHANNELS);
    assert_eq!(
        event.channels().acquisition,
        AcquisitionChannel::ExternalOrganicSearch
    );
    assert_eq!(
        event.channels().discovery,
        DiscoveryChannel::OrganicRecommendation
    );
    assert_eq!(event.fact(), VerifiedFact::ChapterFinished);
    assert_eq!(event.event_id(), [1; 16]);
    assert_ne!(
        DiscoveryChannel::SiteSearch,
        DiscoveryChannel::OrganicRecommendation
    );
    assert_ne!(
        DiscoveryChannel::PaidInternalAdvertisement,
        DiscoveryChannel::OrganicRecommendation
    );
    assert_ne!(
        AcquisitionChannel::ExternalReferral,
        AcquisitionChannel::DirectOrUnknown
    );
}

#[test]
fn flow_01_search_to_recommendation_to_read_is_one_conversion_even_after_replay() {
    let event = finish(7, CHANNELS);
    assert_eq!(count_canonical_conversions(&[event, event, event]), Ok(1));
    let changed = finish(
        7,
        Channels {
            acquisition: CHANNELS.acquisition,
            discovery: DiscoveryChannel::PaidInternalAdvertisement,
        },
    );
    assert_eq!(
        count_canonical_conversions(&[event, changed]),
        Err(FlowContractError::ConflictingCanonicalEvidence),
        "re-attributing the same committed event must not create extra conversions"
    );
}

#[test]
fn flow_01_ten_chapter_events_from_one_reader_are_not_ten_unique_readers() {
    assert_eq!(
        Metric::RegisteredReadersUnique.period_aggregation(),
        PeriodAggregation::DistinctRegisteredReaders
    );
    for metric in [
        Metric::HttpRequests,
        Metric::PageViews,
        Metric::VisitStarts,
        Metric::AnonymousSessions,
        Metric::BookOpens,
        Metric::ChapterOpens,
        Metric::VerifiedChapterFinishes,
        Metric::BillableAdImpressions,
    ] {
        assert_eq!(
            metric.period_aggregation(),
            PeriodAggregation::SumDeduplicatedEvents,
            "{metric:?}"
        );
    }
    let completions: Vec<_> = (1..=10).map(|id| finish(id, CHANNELS)).collect();
    assert_eq!(count_canonical_conversions(&completions), Ok(10));
    // One reader can finish ten chapters; only the registered-reader metric
    // applies period-wide DISTINCT, never the sum of daily or chapter uniques.
    assert_eq!(
        Metric::RegisteredReadersUnique.period_aggregation(),
        PeriodAggregation::DistinctRegisteredReaders
    );
}

#[test]
fn flow_01_untrusted_client_or_http_request_cannot_claim_domain_result() {
    let facts = [
        (
            VerifiedFact::BookOpened,
            FactAuthority::AuthorizedBookNavigation,
        ),
        (
            VerifiedFact::ChapterStarted,
            FactAuthority::ReaderTransaction,
        ),
        (
            VerifiedFact::ChapterFinished,
            FactAuthority::ReaderTransaction,
        ),
        (
            VerifiedFact::LibrarySaved,
            FactAuthority::LibraryTransaction,
        ),
        (
            VerifiedFact::PurchaseCommitted,
            FactAuthority::CommerceTransaction,
        ),
        (
            VerifiedFact::AdImpressionBilled,
            FactAuthority::AdLedgerTransaction,
        ),
    ];
    for (fact, authority) in facts {
        assert!(VerifiedConversion::new([1; 16], fact, authority, CHANNELS).is_ok());
        for forbidden in [
            FactAuthority::ClientClaim,
            FactAuthority::TechnicalTelemetry,
        ] {
            assert_eq!(
                VerifiedConversion::new([1; 16], fact, forbidden, CHANNELS),
                Err(FlowContractError::UntrustedFactAuthority)
            );
        }
    }
    assert_eq!(
        VerifiedConversion::new(
            [0; 16],
            VerifiedFact::ChapterFinished,
            FactAuthority::ReaderTransaction,
            CHANNELS
        ),
        Err(FlowContractError::MissingCanonicalEventId)
    );
    assert_eq!(
        VerifiedConversion::new(
            [1; 16],
            VerifiedFact::PurchaseCommitted,
            FactAuthority::ReaderTransaction,
            CHANNELS
        ),
        Err(FlowContractError::UntrustedFactAuthority)
    );
}

#[test]
fn flow_01_reconciliation_is_bounded_and_conflicting_evidence_fails() {
    let first = finish(1, CHANNELS);
    assert_eq!(count_canonical_conversions(&[first, first]), Ok(1));
    let conflicting = VerifiedConversion::new(
        [1; 16],
        VerifiedFact::ChapterStarted,
        FactAuthority::ReaderTransaction,
        CHANNELS,
    )
    .unwrap();
    assert_eq!(
        count_canonical_conversions(&[first, conflicting]),
        Err(FlowContractError::ConflictingCanonicalEvidence)
    );
    let too_many = vec![first; MAX_CONVERSION_RECONCILIATION_ROWS + 1];
    assert_eq!(
        count_canonical_conversions(&too_many),
        Err(FlowContractError::OversizedReconciliation)
    );
}

#[test]
fn flow_02_unclassified_html_requests_are_not_verified_funnel_visitors() {
    let mut evidence = page(RouteSurface::BookDetail);
    evidence.hygiene = TrafficHygiene::Unclassified;
    assert!(evidence.observed_html_request());
    assert!(!evidence.eligible_for_funnel());
    evidence.hygiene = TrafficHygiene::SuspectedAutomation;
    assert!(!evidence.observed_html_request());
    assert!(!evidence.eligible_for_funnel());
    evidence.hygiene = TrafficHygiene::Eligible;
    assert!(evidence.eligible_for_funnel());
}
