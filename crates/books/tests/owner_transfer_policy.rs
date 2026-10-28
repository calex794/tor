use tor_books::services::owner_transfer_policy::{
    authorize_owner_transfer, AccessTier, LayerTransferState, OwnerTransferDenial,
    OwnerTransferFacts, RevenueTransferState,
};

fn verified_basic() -> OwnerTransferFacts {
    OwnerTransferFacts {
        actor_id: 11,
        book_id: 50,
        expected_owner_id: 11,
        actual_owner_id: 11,
        recipient_id: 12,
        active_owner_member_id: Some(11),
        book_blocked_or_deleted: false,
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
            book_blocked_or_deleted: true,
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
