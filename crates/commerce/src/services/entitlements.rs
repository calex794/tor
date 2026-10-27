//! D-03: direct-purchase access policy; no reader position is an entitlement.
use tor_contracts::paid_delivery::{
    ChapterVersionRef, DenialReason, EntitlementDecision, PositiveId,
};

/// Source facts must be read from an authoritative purchase and revocation join.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectPurchaseRecord {
    pub purchased_version_id: i64,
    pub revoked: bool,
    pub admin_hold: bool,
    pub hard_deleted: bool,
    pub canonical_readable: bool,
}

/// Hard removal and moderation override a purchase; only the pinned edition
/// is ever authorized. Store errors are not mapped into a permit decision.
pub fn decide_direct_purchase(
    chapter_id: PositiveId,
    record: Option<DirectPurchaseRecord>,
) -> EntitlementDecision {
    let Some(record) = record else {
        return EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased,
        };
    };
    if record.admin_hold {
        return EntitlementDecision::Denied {
            reason: DenialReason::Locked,
        };
    }
    if record.hard_deleted {
        return EntitlementDecision::Denied {
            reason: DenialReason::Hidden,
        };
    }
    if record.revoked {
        return EntitlementDecision::Denied {
            reason: DenialReason::Refunded,
        };
    }
    if !record.canonical_readable {
        return EntitlementDecision::Denied {
            reason: DenialReason::Unknown,
        };
    }
    let Ok(version_id) = PositiveId::new(record.purchased_version_id) else {
        return EntitlementDecision::Denied {
            reason: DenialReason::Unknown,
        };
    };
    EntitlementDecision::Allowed {
        pinned: ChapterVersionRef {
            chapter_id,
            version_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> DirectPurchaseRecord {
        DirectPurchaseRecord {
            purchased_version_id: 7,
            revoked: false,
            admin_hold: false,
            hard_deleted: false,
            canonical_readable: true,
        }
    }

    #[test]
    fn d_03_policy_only_permits_confirmed_pinned_version() {
        let id = PositiveId::new(3).unwrap();
        assert_eq!(
            decide_direct_purchase(id, None),
            EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased
            }
        );
        assert_eq!(
            decide_direct_purchase(id, Some(record())),
            EntitlementDecision::Allowed {
                pinned: ChapterVersionRef {
                    chapter_id: id,
                    version_id: PositiveId::new(7).unwrap()
                }
            }
        );
        assert_eq!(
            decide_direct_purchase(
                id,
                Some(DirectPurchaseRecord {
                    canonical_readable: false,
                    ..record()
                })
            ),
            EntitlementDecision::Denied {
                reason: DenialReason::Unknown
            }
        );
        assert_eq!(
            decide_direct_purchase(
                id,
                Some(DirectPurchaseRecord {
                    purchased_version_id: 0,
                    ..record()
                })
            ),
            EntitlementDecision::Denied {
                reason: DenialReason::Unknown
            }
        );
    }

    #[test]
    fn d_03_hard_denials_override_existing_paid_pin() {
        let id = PositiveId::new(3).unwrap();
        for (state, expected) in [
            (
                DirectPurchaseRecord {
                    revoked: true,
                    ..record()
                },
                DenialReason::Refunded,
            ),
            (
                DirectPurchaseRecord {
                    admin_hold: true,
                    ..record()
                },
                DenialReason::Locked,
            ),
            (
                DirectPurchaseRecord {
                    hard_deleted: true,
                    ..record()
                },
                DenialReason::Hidden,
            ),
            (
                DirectPurchaseRecord {
                    revoked: true,
                    admin_hold: true,
                    ..record()
                },
                DenialReason::Locked,
            ),
        ] {
            assert_eq!(
                decide_direct_purchase(id, Some(state)),
                EntitlementDecision::Denied { reason: expected }
            );
        }
    }
}
