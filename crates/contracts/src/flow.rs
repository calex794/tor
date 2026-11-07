//! FLOW-01: typed audience measurement semantics shared across features.
//! This is NOT an event collector, visitor identity provider, or billing guard.
//! Domain owners must establish the asserted provenance before using these types.
use std::collections::BTreeMap;

/// Versioned measurement contract. A changed meaning needs a new version.
pub const FLOW_CONTRACT_VERSION: &str = "flow_v1";
/// A visit expires after 30 minutes without observed activity (FLOW-02).
pub const VISIT_IDLE_TIMEOUT_SECONDS: u64 = 30 * 60;
/// Bounds in-memory reconciliation; durable deduplication belongs to PostgreSQL.
pub const MAX_CONVERSION_RECONCILIATION_ROWS: usize = 1_024;

/// Attribution for entering TOR. None of these values grants entitlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquisitionChannel {
    ExternalOrganicSearch,
    ExternalReferral,
    ExternalPaid,
    /// A missing/blocked referrer does not prove a direct visit.
    DirectOrUnknown,
}

/// Attribution for selecting a book *inside* TOR, not for acquiring a visit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryChannel {
    OrganicRecommendation,
    PaidInternalAdvertisement,
    Catalogue,
    SiteSearch,
    LibraryOrFollow,
    DirectBookOrUnknown,
}

/// A single visit may have both an external acquisition and an internal touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channels {
    pub acquisition: AcquisitionChannel,
    pub discovery: DiscoveryChannel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestMethod {
    Get,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseKind {
    Html,
    Other,
}

/// Normalized, server-registered route class. Never an arbitrary URL or query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteSurface {
    Home,
    Catalogue,
    BookDetail,
    ChapterReader,
    AuthenticatedPage,
    StaticAsset,
    Api,
    Ops,
    Unknown,
}

/// Eligible means not filtered as automation, NOT proven to be a unique human.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrafficHygiene {
    Eligible,
    Unclassified,
    SuspectedAutomation,
}

/// Inputs must come from the trusted router/handler after visibility checks.
/// No cookie, IP, raw URL, user ID or chapter body is part of this contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageViewEvidence {
    pub method: RequestMethod,
    pub status: u16,
    pub response_kind: ResponseKind,
    pub surface: RouteSurface,
    pub hygiene: TrafficHygiene,
    pub visibility_confirmed: bool,
}

impl PageViewEvidence {
    /// Safe low-cardinality count of *HTTP responses*, not verified humans.
    /// Unclassified visitors may be counted here, but never as unique people.
    pub fn observed_html_request(self) -> bool {
        let page = matches!(
            self.surface,
            RouteSurface::Home
                | RouteSurface::Catalogue
                | RouteSurface::BookDetail
                | RouteSurface::ChapterReader
                | RouteSurface::AuthenticatedPage
        );
        self.method == RequestMethod::Get
            && self.status == 200
            && self.response_kind == ResponseKind::Html
            && self.hygiene != TrafficHygiene::SuspectedAutomation
            && self.visibility_confirmed
            && page
    }

    /// Verified/opted-in subset, not proof of biological human attention.
    pub fn eligible_for_funnel(self) -> bool {
        self.observed_html_request() && self.hygiene == TrafficHygiene::Eligible
    }
}

/// The aggregation policy is part of the metric's definition, not UI styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    HttpRequests,
    PageViews,
    VisitStarts,
    AnonymousSessions,
    BookOpens,
    ChapterOpens,
    VerifiedChapterFinishes,
    RegisteredReadersUnique,
    BillableAdImpressions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodAggregation {
    /// Count once per canonical event, then sum disjoint time buckets.
    SumDeduplicatedEvents,
    /// Must use a distinct verified reader set across the ENTIRE period.
    DistinctRegisteredReaders,
}

impl Metric {
    pub fn period_aggregation(self) -> PeriodAggregation {
        match self {
            Self::RegisteredReadersUnique => PeriodAggregation::DistinctRegisteredReaders,
            _ => PeriodAggregation::SumDeduplicatedEvents,
        }
    }
}

/// These are asserted by the owning server domain, never by a client parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactAuthority {
    AuthorizedBookNavigation,
    ReaderTransaction,
    LibraryTransaction,
    CommerceTransaction,
    AdLedgerTransaction,
    ClientClaim,
    TechnicalTelemetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifiedFact {
    BookOpened,
    ChapterStarted,
    ChapterFinished,
    LibrarySaved,
    PurchaseCommitted,
    AdImpressionBilled,
}

impl VerifiedFact {
    pub fn accepts(self, authority: FactAuthority) -> bool {
        matches!(
            (self, authority),
            (Self::BookOpened, FactAuthority::AuthorizedBookNavigation)
                | (Self::ChapterStarted, FactAuthority::ReaderTransaction)
                | (Self::ChapterFinished, FactAuthority::ReaderTransaction)
                | (Self::LibrarySaved, FactAuthority::LibraryTransaction)
                | (Self::PurchaseCommitted, FactAuthority::CommerceTransaction)
                | (Self::AdImpressionBilled, FactAuthority::AdLedgerTransaction)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowContractError {
    MissingCanonicalEventId,
    UntrustedFactAuthority,
    OversizedReconciliation,
    ConflictingCanonicalEvidence,
}

/// A canonical committed fact with TWO separate attribution dimensions.
/// The caller must first verify the domain transaction and event ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedConversion {
    event_id: [u8; 16],
    fact: VerifiedFact,
    channels: Channels,
}

impl VerifiedConversion {
    pub fn new(
        event_id: [u8; 16],
        fact: VerifiedFact,
        authority: FactAuthority,
        channels: Channels,
    ) -> Result<Self, FlowContractError> {
        if event_id == [0; 16] {
            return Err(FlowContractError::MissingCanonicalEventId);
        }
        if !fact.accepts(authority) {
            return Err(FlowContractError::UntrustedFactAuthority);
        }
        Ok(Self {
            event_id,
            fact,
            channels,
        })
    }

    pub fn event_id(&self) -> [u8; 16] {
        self.event_id
    }

    pub fn fact(&self) -> VerifiedFact {
        self.fact
    }

    pub fn channels(&self) -> Channels {
        self.channels
    }
}

/// An independently bounded reconciliation primitive, not the production
/// analytics store. Two source dimensions do not create two conversions.
/// Conflicting channel assignments for one canonical event fail closed.
pub fn count_canonical_conversions(
    conversions: &[VerifiedConversion],
) -> Result<usize, FlowContractError> {
    if conversions.len() > MAX_CONVERSION_RECONCILIATION_ROWS {
        return Err(FlowContractError::OversizedReconciliation);
    }
    let mut seen = BTreeMap::new();
    for conversion in conversions {
        let evidence = (conversion.fact, conversion.channels);
        if let Some(previous) = seen.insert(conversion.event_id, evidence) {
            if previous != evidence {
                return Err(FlowContractError::ConflictingCanonicalEvidence);
            }
        }
    }
    Ok(seen.len())
}
