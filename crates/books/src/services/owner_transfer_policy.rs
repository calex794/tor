//! B-09b / S030: fail-closed ownership transfer eligibility.
//!
//! This is a pure policy, NOT an executable permission grant. The future
//! transfer use case must load these facts from trusted A (account tier), B
//! (Book / active owner), D (all historical Layer entitlements) and B-10
//! (immutable economic schedules) repositories under its transaction guard.
//! HTTP arguments must never populate these authority fields.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AccessTier {
    Basic,
    Adult,
    ProAdult,
}

impl AccessTier {
    pub fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "basic" => Some(Self::Basic),
            "adult" => Some(Self::Adult),
            "pro_adult" => Some(Self::ProAdult),
            _ => None,
        }
    }
}

/// "Clear" must inspect active entitlements inherited from every Layer
/// version, including rights retained after the Book leaves live composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerTransferState {
    Unverified,
    ActiveOrHistoricalEntitlement,
    VerifiedClearIncludingHistorical,
}

/// Ownership must never rewrite a revenue-share snapshot of an existing sale.
/// Unknown B-10/D financial provenance is a hard stop, not a default to zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevenueTransferState {
    Unverified,
    WouldRewriteHistoricalPurchase,
    HistoricalPurchasesPreserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerTransferDenial {
    InvalidActorOrRecipient,
    StaleOrUnverifiedOwner,
    BookHeldOrUnavailable,
    UnknownBookTier,
    UnknownRecipientTier,
    RecipientTierTooLow,
    UnverifiedLayerEntitlements,
    ExistingLayerEntitlement,
    UnverifiedRevenueSnapshots,
    RevenueSnapshotsWouldChange,
}

/// These fields are a contract for *server-verified* facts, not transport
/// request fields. A matching role label or Team role alone cannot authorize
/// transfer. A PostgreSQL CAS and owner-member swap are still required.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerTransferFacts {
    pub actor_id: i64,
    pub book_id: i64,
    pub expected_owner_id: i64,
    pub actual_owner_id: i64,
    pub recipient_id: i64,
    pub active_owner_member_id: Option<i64>,
    pub book_blocked_or_deleted: bool,
    pub book_tier: Option<AccessTier>,
    pub recipient_effective_tier: Option<AccessTier>,
    pub layer_state: LayerTransferState,
    pub revenue_state: RevenueTransferState,
}

/// The only successful result permits the caller to *attempt* the B-owned
/// transaction; it is never an HTTP/S3/paid-text permission itself.
pub fn authorize_owner_transfer(facts: OwnerTransferFacts) -> Result<(), OwnerTransferDenial> {
    if facts.actor_id <= 0
        || facts.book_id <= 0
        || facts.expected_owner_id <= 0
        || facts.actual_owner_id <= 0
        || facts.recipient_id <= 0
        || facts.recipient_id == facts.actual_owner_id
    {
        return Err(OwnerTransferDenial::InvalidActorOrRecipient);
    }
    if facts.actor_id != facts.expected_owner_id
        || facts.actual_owner_id != facts.expected_owner_id
        || facts.active_owner_member_id != Some(facts.expected_owner_id)
    {
        return Err(OwnerTransferDenial::StaleOrUnverifiedOwner);
    }
    if facts.book_blocked_or_deleted {
        return Err(OwnerTransferDenial::BookHeldOrUnavailable);
    }
    let book_tier = facts.book_tier.ok_or(OwnerTransferDenial::UnknownBookTier)?;
    let recipient_tier = facts
        .recipient_effective_tier
        .ok_or(OwnerTransferDenial::UnknownRecipientTier)?;
    if recipient_tier < book_tier {
        return Err(OwnerTransferDenial::RecipientTierTooLow);
    }
    match facts.layer_state {
        LayerTransferState::Unverified => {
            return Err(OwnerTransferDenial::UnverifiedLayerEntitlements);
        }
        LayerTransferState::ActiveOrHistoricalEntitlement => {
            return Err(OwnerTransferDenial::ExistingLayerEntitlement);
        }
        LayerTransferState::VerifiedClearIncludingHistorical => {}
    }
    match facts.revenue_state {
        RevenueTransferState::Unverified => {
            return Err(OwnerTransferDenial::UnverifiedRevenueSnapshots);
        }
        RevenueTransferState::WouldRewriteHistoricalPurchase => {
            return Err(OwnerTransferDenial::RevenueSnapshotsWouldChange);
        }
        RevenueTransferState::HistoricalPurchasesPreserved => {}
    }
    Ok(())
}
