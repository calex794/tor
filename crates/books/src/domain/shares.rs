//! B-10: atomic economics are separate from Book roles and staffing.
//! Shares describe the *creator pool*, not the platform fee or a payout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShareAssignment {
    pub user_id: i64,
    pub share_percent: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevenueShareError {
    Invalid,
    Forbidden,
    Missing,
    Conflict,
    InactiveRecipient,
    Unavailable,
}

/// Normalise the COMPLETE intended schedule. Omitted users have 0%.
/// No direct per-member financial mutation is permitted.
pub fn validate_schedule(
    assignments: &[ShareAssignment],
) -> Result<Vec<ShareAssignment>, RevenueShareError> {
    if assignments.is_empty() || assignments.len() > 100 {
        return Err(RevenueShareError::Invalid);
    }
    let mut sorted = assignments.to_vec();
    sorted.sort_unstable_by_key(|row| row.user_id);
    let mut total = 0i32;
    let mut previous = 0i64;
    for row in &sorted {
        if row.user_id <= 0 || row.user_id == previous || !(1..=100).contains(&row.share_percent) {
            return Err(RevenueShareError::Invalid);
        }
        previous = row.user_id;
        total += i32::from(row.share_percent);
    }
    if total != 100 {
        return Err(RevenueShareError::Invalid);
    }
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_10_schedule_is_a_canonical_100_percent_batch() {
        assert_eq!(
            validate_schedule(&[
                ShareAssignment {
                    user_id: 8,
                    share_percent: 20
                },
                ShareAssignment {
                    user_id: 2,
                    share_percent: 80
                }
            ]),
            Ok(vec![
                ShareAssignment {
                    user_id: 2,
                    share_percent: 80
                },
                ShareAssignment {
                    user_id: 8,
                    share_percent: 20
                }
            ])
        );
    }

    #[test]
    fn b_10_rejects_invalid_duplicates_and_intermediate_totals() {
        for invalid in [
            vec![],
            vec![ShareAssignment {
                user_id: 2,
                share_percent: 80,
            }],
            vec![ShareAssignment {
                user_id: 2,
                share_percent: 110,
            }],
            vec![ShareAssignment {
                user_id: 0,
                share_percent: 100,
            }],
            vec![
                ShareAssignment {
                    user_id: 2,
                    share_percent: 0,
                },
                ShareAssignment {
                    user_id: 3,
                    share_percent: 100,
                },
            ],
            vec![
                ShareAssignment {
                    user_id: 2,
                    share_percent: 50,
                },
                ShareAssignment {
                    user_id: 2,
                    share_percent: 50,
                },
            ],
        ] {
            assert_eq!(validate_schedule(&invalid), Err(RevenueShareError::Invalid));
        }
    }
}
