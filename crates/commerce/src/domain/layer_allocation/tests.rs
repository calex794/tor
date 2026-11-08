use super::*;

fn base() -> AttributionInputs {
    AttributionInputs {
        payment_id: 71,
        term_id: 42,
        gross_minor: 101,
        currency: "RUB".into(),
        policy_id: Some("finance-reviewed-example-v1".into()),
        platform_fee_minor: Some(20),
        weighting: Some(BookWeighting::WeightedLargestRemainder),
        frozen_schedule_verified: true,
        allocation_basis_confirmed: true,
        // Intentionally unsorted: SQL and caller iteration order cannot
        // influence the result of any exact-minor attribution.
        books: vec![
            FrozenBook {
                book_id: 20,
                share_revision: 1,
                weight_bps: Some(5_000),
                participants: vec![FrozenParticipant {
                    user_id: 300,
                    percent: 100,
                }],
            },
            FrozenBook {
                book_id: 10,
                share_revision: 2,
                weight_bps: Some(5_000),
                participants: vec![
                    FrozenParticipant {
                        user_id: 200,
                        percent: 30,
                    },
                    FrozenParticipant {
                        user_id: 100,
                        percent: 70,
                    },
                ],
            },
        ],
    }
}

#[test]
fn d_08_weighted_books_and_frozen_members_conserve_every_minor() {
    let preview = preview_layer_attribution(&base()).unwrap();
    assert_eq!(preview.policy_id, "finance-reviewed-example-v1");
    assert_eq!(preview.platform_fee_minor, 20);
    assert_eq!(
        preview.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![10, 20]
    );
    assert_eq!(
        preview
            .books
            .iter()
            .map(|b| b.amount_minor)
            .collect::<Vec<_>>(),
        vec![41, 40]
    );
    assert_eq!(preview.books[0].share_revision, 2);
    assert_eq!(
        preview.books[0].participants,
        vec![
            ParticipantAmount {
                user_id: 100,
                amount_minor: 29
            },
            ParticipantAmount {
                user_id: 200,
                amount_minor: 12
            },
        ]
    );
    assert_eq!(
        preview.books[1].participants,
        vec![ParticipantAmount {
            user_id: 300,
            amount_minor: 40
        }]
    );
    assert_eq!(preview.attributed_minor(), 101);
}

#[test]
fn d_08_nevboo_legacy_equal_split_residue_goes_to_last_book() {
    let mut input = base();
    input.weighting = Some(BookWeighting::LegacyEqualLast);
    input.platform_fee_minor = Some(0);
    for book in &mut input.books {
        book.weight_bps = None;
    }
    let preview = preview_layer_attribution(&input).unwrap();
    assert_eq!(preview.books[0].amount_minor, 50);
    assert_eq!(preview.books[1].amount_minor, 51);
    assert_eq!(preview.attributed_minor(), 101);
    // The legacy policy is explicitly selected, not silently guessed.
    input.weighting = None;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
}

#[test]
fn d_08_largest_remainder_ties_are_stable_by_book_and_user_id() {
    let mut input = base();
    input.gross_minor = 1;
    input.platform_fee_minor = Some(0);
    input.books[1].participants = vec![
        FrozenParticipant {
            user_id: 200,
            percent: 50,
        },
        FrozenParticipant {
            user_id: 100,
            percent: 50,
        },
    ];
    let result = preview_layer_attribution(&input).unwrap();
    assert_eq!(result.books[0].amount_minor, 1);
    assert_eq!(result.books[1].amount_minor, 0);
    assert_eq!(result.books[0].participants[0].amount_minor, 1);
    assert_eq!(result.books[0].participants[1].amount_minor, 0);
}

#[test]
fn d_08_policy_fee_and_verified_snapshots_are_not_optional_defaults() {
    let mut input = base();
    input.policy_id = None;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
    input = base();
    input.platform_fee_minor = None;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
    input = base();
    input.frozen_schedule_verified = false;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
    input = base();
    input.allocation_basis_confirmed = false;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
    input = base();
    input.weighting = None;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
}

#[test]
fn d_08_rejects_unknown_book_weights_or_invalid_team_percentages() {
    let mut input = base();
    input.books[0].weight_bps = Some(4_999);
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidWeights)
    );
    input = base();
    input.books[0].weight_bps = None;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidWeights)
    );
    input = base();
    input.books[1].participants[0].percent = 20;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::IncompleteShareSnapshot)
    );
    input = base();
    let duplicate = input.books[1].participants[0].clone();
    input.books[1].participants.push(duplicate);
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::IncompleteShareSnapshot)
    );
    input = base();
    input.books[1].book_id = 20;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidBookSet)
    );
}

#[test]
fn d_08_invalid_amount_currency_and_reference_never_allocate() {
    let mut input = base();
    input.gross_minor = 0;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidCharge)
    );
    input = base();
    input.platform_fee_minor = Some(102);
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidCharge)
    );
    input = base();
    input.currency = "USD".into();
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidCharge)
    );
    input = base();
    input.payment_id = 0;
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::InvalidCharge)
    );
    input = base();
    input.policy_id = Some("".into());
    assert_eq!(
        preview_layer_attribution(&input),
        Err(AttributionError::UnconfirmedPolicy)
    );
}

#[test]
fn d_08_huge_i64_charge_uses_i128_without_float_or_overflow() {
    let mut input = base();
    input.gross_minor = i64::MAX;
    input.platform_fee_minor = Some(0);
    let preview = preview_layer_attribution(&input).unwrap();
    assert_eq!(preview.attributed_minor(), i128::from(i64::MAX));
    assert_eq!(
        preview
            .books
            .iter()
            .map(|b| i128::from(b.amount_minor))
            .sum::<i128>(),
        i128::from(i64::MAX)
    );
}

#[test]
fn d_08_full_refund_is_an_unposted_negative_mirror_of_original() {
    let original = preview_layer_attribution(&base()).unwrap();
    let snapshot = original.clone();
    let mirrored = original.full_reversal_draft();
    assert_eq!(mirrored.len(), 4);
    assert_eq!(
        mirrored
            .iter()
            .map(|line| i128::from(line.delta_minor))
            .sum::<i128>(),
        -i128::from(original.gross_minor)
    );
    assert_eq!(original, snapshot, "no rewrite of purchase provenance");
    assert_eq!(mirrored[1].share_revision, Some(2));
}

#[test]
fn d_08_input_order_does_not_change_money_or_recipient_order() {
    let original = base();
    let first = preview_layer_attribution(&original).unwrap();
    let mut swapped = original;
    swapped.books.reverse();
    for book in &mut swapped.books {
        book.participants.reverse();
    }
    let second = preview_layer_attribution(&swapped).unwrap();
    assert_eq!(first, second);
}
