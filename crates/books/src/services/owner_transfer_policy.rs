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
    ActiveEntitlementFromAnyLayerVersion,
    VerifiedClearIncludingHistorical,
}

/// A missing Book lifecycle/hold decision cannot silently become "clear".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookTransferState {
    Unverified,
    Available,
    HeldOrDeleted,
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
    pub(crate) actor_id: i64,
    pub(crate) book_id: i64,
    pub(crate) expected_owner_id: i64,
    pub(crate) actual_owner_id: i64,
    pub(crate) recipient_id: i64,
    pub(crate) active_owner_member_id: Option<i64>,
    pub(crate) book_state: BookTransferState,
    pub(crate) book_tier: Option<AccessTier>,
    pub(crate) recipient_effective_tier: Option<AccessTier>,
    pub(crate) layer_state: LayerTransferState,
    pub(crate) revenue_state: RevenueTransferState,
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
    if facts.book_state != BookTransferState::Available {
        return Err(OwnerTransferDenial::BookHeldOrUnavailable);
    }
    let book_tier = facts
        .book_tier
        .ok_or(OwnerTransferDenial::UnknownBookTier)?;
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
        LayerTransferState::ActiveEntitlementFromAnyLayerVersion => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn verified_basic() -> OwnerTransferFacts {
        OwnerTransferFacts {
            actor_id: 11,
            book_id: 50,
            expected_owner_id: 11,
            actual_owner_id: 11,
            recipient_id: 12,
            active_owner_member_id: Some(11),
            book_state: BookTransferState::Available,
            book_tier: Some(AccessTier::Basic),
            recipient_effective_tier: Some(AccessTier::Basic),
            layer_state: LayerTransferState::VerifiedClearIncludingHistorical,
            revenue_state: RevenueTransferState::HistoricalPurchasesPreserved,
        }
    }

    #[test]
    fn b_09_transfer_tiers_are_monotonic_including_pro_adult_on_adult_book() {
        let levels = [AccessTier::Basic, AccessTier::Adult, AccessTier::ProAdult];
        for (book_index, book) in levels.iter().enumerate() {
            for (recipient_index, recipient) in levels.iter().enumerate() {
                let facts = OwnerTransferFacts {
                    book_tier: Some(*book),
                    recipient_effective_tier: Some(*recipient),
                    ..verified_basic()
                };
                let expected = if recipient_index >= book_index {
                    Ok(())
                } else {
                    Err(OwnerTransferDenial::RecipientTierTooLow)
                };
                assert_eq!(authorize_owner_transfer(facts), expected);
            }
        }
        assert_eq!(
            AccessTier::from_persisted("pro_adult"),
            Some(AccessTier::ProAdult)
        );
        assert_eq!(AccessTier::from_persisted("adult"), Some(AccessTier::Adult));
        assert_eq!(AccessTier::from_persisted("basic"), Some(AccessTier::Basic));
        assert_eq!(AccessTier::from_persisted("untrusted"), None);
    }

    #[test]
    fn b_09_transfer_denies_unknown_tiers_and_unverified_layer_history() {
        let base = verified_basic();
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                book_tier: None,
                ..base
            }),
            Err(OwnerTransferDenial::UnknownBookTier)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                recipient_effective_tier: None,
                ..base
            }),
            Err(OwnerTransferDenial::UnknownRecipientTier)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                layer_state: LayerTransferState::Unverified,
                ..base
            }),
            Err(OwnerTransferDenial::UnverifiedLayerEntitlements)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                layer_state: LayerTransferState::ActiveEntitlementFromAnyLayerVersion,
                ..base
            }),
            Err(OwnerTransferDenial::ExistingLayerEntitlement),
            "removed from live Layer is not enough: historical active holders block transfer"
        );
    }

    #[test]
    fn b_09_transfer_preserves_historical_revenue_provenance() {
        let base = verified_basic();
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                revenue_state: RevenueTransferState::Unverified,
                ..base
            }),
            Err(OwnerTransferDenial::UnverifiedRevenueSnapshots)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                revenue_state: RevenueTransferState::WouldRewriteHistoricalPurchase,
                ..base
            }),
            Err(OwnerTransferDenial::RevenueSnapshotsWouldChange)
        );
    }

    #[test]
    fn b_09_transfer_denies_spoofed_owner_stale_cas_held_book_and_self_transfer() {
        let base = verified_basic();
        for invalid in [
            OwnerTransferFacts {
                actor_id: 99,
                ..base
            },
            OwnerTransferFacts {
                actual_owner_id: 14,
                ..base
            },
            OwnerTransferFacts {
                active_owner_member_id: None,
                ..base
            },
        ] {
            assert_eq!(
                authorize_owner_transfer(invalid),
                Err(OwnerTransferDenial::StaleOrUnverifiedOwner)
            );
        }
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                book_state: BookTransferState::HeldOrDeleted,
                ..base
            }),
            Err(OwnerTransferDenial::BookHeldOrUnavailable)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                book_state: BookTransferState::Unverified,
                ..base
            }),
            Err(OwnerTransferDenial::BookHeldOrUnavailable)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                recipient_id: 11,
                ..base
            }),
            Err(OwnerTransferDenial::InvalidActorOrRecipient)
        );
        assert_eq!(
            authorize_owner_transfer(OwnerTransferFacts {
                actor_id: -1,
                ..base
            }),
            Err(OwnerTransferDenial::InvalidActorOrRecipient)
        );
    }
}
