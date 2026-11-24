//! Pure integer apportionment, not a finance policy or ledger writer.

#[derive(Clone, Copy)]
struct RemainderRank {
    index: usize,
    remainder: i128,
    recipient_id: i64,
}

/// Split a nonnegative pool by largest fractional remainder. The caller
/// validates unique recipient IDs and its domain-specific share policy.
/// The result retains input order; equal remainders prefer the lower ID.
/// Products use i128, never floats or rounded intermediate money values.
pub(super) fn allocate(
    total_minor: i64,
    weights: &[(i64, i64)],
    denominator: i128,
) -> Option<Vec<i64>> {
    if total_minor < 0 || denominator <= 0 || weights.is_empty() {
        return None;
    }
    let mut weight_sum = 0_i128;
    let mut amounts = Vec::with_capacity(weights.len());
    let mut ranking = Vec::with_capacity(weights.len());
    let mut distributed = 0_i128;
    for (index, &(recipient_id, weight)) in weights.iter().enumerate() {
        if recipient_id <= 0 || weight <= 0 {
            return None;
        }
        weight_sum = weight_sum.checked_add(i128::from(weight))?;
        let product = i128::from(total_minor) * i128::from(weight);
        let base = i64::try_from(product / denominator).ok()?;
        distributed = distributed.checked_add(i128::from(base))?;
        amounts.push(base);
        ranking.push(RemainderRank {
            index,
            remainder: product % denominator,
            recipient_id,
        });
    }
    if weight_sum != denominator {
        return None;
    }
    let remaining = usize::try_from(i128::from(total_minor) - distributed).ok()?;
    if remaining > ranking.len() {
        return None;
    }
    ranking.sort_by(|left, right| {
        right
            .remainder
            .cmp(&left.remainder)
            .then_with(|| left.recipient_id.cmp(&right.recipient_id))
    });
    for rank in ranking.into_iter().take(remaining) {
        amounts[rank.index] = amounts[rank.index].checked_add(1)?;
    }
    Some(amounts)
}

#[cfg(test)]
mod tests {
    use super::allocate;

    #[test]
    fn all_two_recipient_percentage_splits_match_integer_oracle() {
        // For two recipients the lower ID receives its rounded half-up
        // quota, including equal remainders. The other gets the residue.
        for total in 0_i64..=101 {
            for weight in 1_i64..100 {
                let lower = (total * weight + 50) / 100;
                let higher = total - lower;
                assert_eq!(
                    allocate(total, &[(10, weight), (20, 100 - weight)], 100),
                    Some(vec![lower, higher])
                );
                assert_eq!(
                    allocate(total, &[(20, 100 - weight), (10, weight)], 100),
                    Some(vec![higher, lower])
                );
            }
        }
    }

    #[test]
    fn tie_break_and_largest_remainder_are_independent_of_input_order() {
        assert_eq!(
            allocate(2, &[(30, 34), (20, 33), (10, 33)], 100),
            Some(vec![1, 0, 1])
        );
        assert_eq!(
            allocate(2, &[(10, 33), (30, 34), (20, 33)], 100),
            Some(vec![1, 1, 0])
        );
    }

    #[test]
    fn basis_points_and_maximum_pool_keep_every_kopeck() {
        let amounts = allocate(i64::MAX, &[(20, 5000), (10, 5000)], 10_000).unwrap();
        assert_eq!(amounts, vec![i64::MAX / 2, i64::MAX / 2 + 1]);
        assert_eq!(amounts.iter().sum::<i64>(), i64::MAX);
        assert_eq!(allocate(0, &[(7, 100)], 100), Some(vec![0]));
    }

    #[test]
    fn invalid_arithmetic_inputs_are_rejected() {
        assert_eq!(allocate(-1, &[(7, 100)], 100), None);
        assert_eq!(allocate(1, &[(7, 100)], 0), None);
        assert_eq!(allocate(1, &[], 100), None);
        assert_eq!(allocate(1, &[(0, 100)], 100), None);
        assert_eq!(allocate(1, &[(7, 0), (8, 100)], 100), None);
        assert_eq!(allocate(1, &[(7, 99)], 100), None);
        assert_eq!(allocate(1, &[(7, 101)], 100), None);
    }
}
