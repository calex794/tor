use tor_commerce::domain::{
    money::MoneyMinor,
    revenue_allocation::{
        calculate_purchase_allocation, CreatorEarningHold, RevenueAllocationError,
        RevenueAllocationPolicy, RevenueShare,
    },
};
use tor_contracts::paid_delivery::PositiveId;

fn policy(version: &str, fee_bps: u16) -> RevenueAllocationPolicy {
    RevenueAllocationPolicy::new(version, fee_bps).unwrap()
}

fn share(user: i64, percent: u8) -> RevenueShare {
    RevenueShare {
        recipient_id: PositiveId::new(user).unwrap(),
        share_percent: percent,
    }
}

fn gross(value: i64) -> MoneyMinor {
    MoneyMinor::new(value).unwrap()
}

#[test]
fn d_08_explicit_policy_conserves_gross_minor_units() {
    let allocation = calculate_purchase_allocation(
        gross(10_000),
        &policy("ci-revenue-20-80-v1", 2_000),
        &[share(11, 50), share(12, 50)],
    )
    .unwrap();

    assert_eq!(allocation.policy_version, "ci-revenue-20-80-v1");
    assert_eq!(allocation.gross_minor.get(), 10_000);
    assert_eq!(allocation.platform_fee_minor.get(), 2_000);
    assert_eq!(allocation.creator_pool_minor.get(), 8_000);
    assert_eq!(
        allocation
            .creators
            .iter()
            .map(|line| line.amount_minor.get())
            .sum::<i64>(),
        8_000
    );
    assert_eq!(allocation.creators[0].amount_minor.get(), 4_000);
    assert_eq!(allocation.creators[1].amount_minor.get(), 4_000);
}

#[test]
fn d_08_platform_fee_uses_half_up_kopeck_rounding() {
    let allocation =
        calculate_purchase_allocation(gross(1), &policy("ci-half-up-v1", 5_000), &[share(7, 100)])
            .unwrap();
    assert_eq!(allocation.platform_fee_minor.get(), 1);
    assert_eq!(allocation.creator_pool_minor.get(), 0);
    assert_eq!(allocation.creators[0].amount_minor.get(), 0);
}

#[test]
fn d_08_largest_remainder_and_user_id_tie_break_are_order_independent() {
    let policy = policy("ci-rounding-v1", 0);
    let forward = calculate_purchase_allocation(
        gross(1),
        &policy,
        &[share(30, 33), share(10, 34), share(20, 33)],
    )
    .unwrap();
    let reverse = calculate_purchase_allocation(
        gross(1),
        &policy,
        &[share(20, 33), share(10, 34), share(30, 33)],
    )
    .unwrap();
    assert_eq!(forward, reverse);
    assert_eq!(forward.creators[0].recipient_id.get(), 10);
    assert_eq!(forward.creators[0].amount_minor.get(), 1);
    assert_eq!(forward.creators[1].amount_minor.get(), 0);
    assert_eq!(forward.creators[2].amount_minor.get(), 0);
}

#[test]
fn d_08_equal_remainders_go_to_lowest_recipient_id() {
    let allocation = calculate_purchase_allocation(
        gross(1),
        &policy("ci-tie-v1", 0),
        &[share(22, 50), share(11, 50)],
    )
    .unwrap();
    assert_eq!(allocation.creators[0].recipient_id.get(), 11);
    assert_eq!(allocation.creators[0].amount_minor.get(), 1);
    assert_eq!(allocation.creators[1].amount_minor.get(), 0);
}

#[test]
fn d_08_hold_copies_exact_purchase_refund_deadline() {
    let allocation = calculate_purchase_allocation(
        gross(10_000),
        &policy("ci-hold-v1", 2_000),
        &[share(11, 50), share(12, 50)],
    )
    .unwrap();
    let holds = allocation
        .creator_holds(PositiveId::new(900).unwrap(), Some(1_800_000_000))
        .unwrap();
    assert_eq!(holds.len(), 2);
    assert_eq!(
        holds,
        vec![
            CreatorEarningHold {
                purchase_id: PositiveId::new(900).unwrap(),
                recipient_id: PositiveId::new(11).unwrap(),
                amount_minor: gross(4_000),
                available_at_unix: 1_800_000_000,
            },
            CreatorEarningHold {
                purchase_id: PositiveId::new(900).unwrap(),
                recipient_id: PositiveId::new(12).unwrap(),
                amount_minor: gross(4_000),
                available_at_unix: 1_800_000_000,
            },
        ]
    );
}

#[test]
fn d_08_missing_refund_deadline_fails_closed_and_zero_earning_needs_no_hold() {
    let allocation = calculate_purchase_allocation(
        gross(100),
        &policy("ci-no-deadline-v1", 0),
        &[share(11, 100)],
    )
    .unwrap();
    assert_eq!(
        allocation.creator_holds(PositiveId::new(901).unwrap(), None),
        Err(RevenueAllocationError::MissingRefundDeadline)
    );

    let platform_only = calculate_purchase_allocation(
        gross(100),
        &policy("ci-platform-only-v1", 10_000),
        &[share(11, 100)],
    )
    .unwrap();
    assert!(platform_only
        .creator_holds(PositiveId::new(902).unwrap(), None)
        .unwrap()
        .is_empty());
}

#[test]
fn d_08_invalid_policy_and_share_schedules_are_rejected() {
    assert_eq!(
        RevenueAllocationPolicy::new(" ", 2_000),
        Err(RevenueAllocationError::InvalidPolicyVersion)
    );
    assert_eq!(
        RevenueAllocationPolicy::new("ci-invalid-fee", 10_001),
        Err(RevenueAllocationError::InvalidPlatformFee)
    );

    let policy = policy("ci-validation-v1", 2_000);
    assert_eq!(
        calculate_purchase_allocation(gross(100), &policy, &[]),
        Err(RevenueAllocationError::EmptyShareSchedule)
    );
    assert_eq!(
        calculate_purchase_allocation(gross(100), &policy, &[share(11, 50)]),
        Err(RevenueAllocationError::SharesMustEqualOneHundred)
    );
    assert_eq!(
        calculate_purchase_allocation(gross(100), &policy, &[share(11, 50), share(11, 50)]),
        Err(RevenueAllocationError::DuplicateRecipient)
    );
    assert_eq!(
        calculate_purchase_allocation(gross(0), &policy, &[share(11, 100)]),
        Err(RevenueAllocationError::ZeroGrossAmount)
    );
}

#[test]
fn d_08_fee_and_creator_pool_are_conserved_for_large_minor_amounts() {
    let allocation = calculate_purchase_allocation(
        gross(i64::MAX),
        &policy("ci-large-v1", 2_000),
        &[share(11, 1), share(12, 99)],
    )
    .unwrap();
    assert_eq!(
        allocation
            .platform_fee_minor
            .checked_add(allocation.creator_pool_minor)
            .unwrap(),
        allocation.gross_minor
    );
    assert_eq!(
        allocation
            .creators
            .iter()
            .map(|line| line.amount_minor.get() as i128)
            .sum::<i128>(),
        i128::from(allocation.creator_pool_minor.get())
    );
}
