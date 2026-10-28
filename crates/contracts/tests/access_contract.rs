use tor_contracts::{
    access::{
        decide_read, AccessDenial, BookMembership, BookRole, ReadGrant, TeamReadingMembership,
        VerifiedReaderFacts,
    },
    paid_delivery::PositiveId,
};
fn id(n: i64) -> PositiveId {
    PositiveId::new(n).unwrap()
}

#[test]
fn acl_01_book_roles_do_not_inherit_team_roles_and_revocation_is_immediate() {
    for role in [BookRole::Owner, BookRole::Translator, BookRole::Moderator] {
        assert!(BookMembership { role, active: true }.can_manage());
        assert!(!BookMembership {
            role,
            active: false
        }
        .can_manage());
        assert_eq!(
            BookMembership { role, active: true }.can_transfer_owner(),
            role == BookRole::Owner
        );
    }
    let active = TeamReadingMembership {
        team_active: true,
        membership_active: true,
        book_link_active: true,
    };
    assert!(active.permits_current());
    for revoked in [
        TeamReadingMembership {
            team_active: false,
            ..active
        },
        TeamReadingMembership {
            membership_active: false,
            ..active
        },
        TeamReadingMembership {
            book_link_active: false,
            ..active
        },
    ] {
        assert!(!revoked.permits_current());
    }
}

#[test]
fn acl_01_hard_block_dominates_all_sources_including_a_paid_purchase() {
    let facts = VerifiedReaderFacts {
        hard_blocked: true,
        direct_purchase_pinned_version: Some(id(11)),
        layer_current_version: Some(id(12)),
        team_current_version: Some(id(13)),
        public_free_current_version: Some(id(14)),
    };
    assert_eq!(decide_read(facts), Err(AccessDenial::HardBlocked));
}

#[test]
fn acl_01_paid_pin_survives_team_revocation_and_never_moves_to_current() {
    let paid = VerifiedReaderFacts {
        direct_purchase_pinned_version: Some(id(100)),
        ..Default::default()
    };
    assert_eq!(
        decide_read(paid),
        Ok(ReadGrant::DirectPurchasePinned {
            version_id: id(100)
        })
    );
    assert_eq!(
        decide_read(VerifiedReaderFacts {
            public_free_current_version: Some(id(200)),
            ..paid
        }),
        Ok(ReadGrant::DirectPurchasePinned {
            version_id: id(100)
        })
    );
}

#[test]
fn acl_01_current_entitlements_are_distinct_and_fail_closed_without_authority() {
    assert_eq!(
        decide_read(VerifiedReaderFacts::default()),
        Err(AccessDenial::Locked)
    );
    assert_eq!(
        decide_read(VerifiedReaderFacts {
            layer_current_version: Some(id(8)),
            ..Default::default()
        }),
        Ok(ReadGrant::LayerCurrent { version_id: id(8) })
    );
    assert_eq!(
        decide_read(VerifiedReaderFacts {
            team_current_version: Some(id(9)),
            ..Default::default()
        }),
        Ok(ReadGrant::TeamCurrent { version_id: id(9) })
    );
    assert_eq!(
        decide_read(VerifiedReaderFacts {
            public_free_current_version: Some(id(10)),
            ..Default::default()
        }),
        Ok(ReadGrant::PublicFreeCurrent { version_id: id(10) })
    );
}
