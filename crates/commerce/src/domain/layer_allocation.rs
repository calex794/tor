//! D-08 / D-05: deterministic, non-posting Layer payment attribution.
//!
//! This is a preview of an EXPLICIT, versioned finance policy, not an
//! earning, a cleared cash credit, a refund or a withdrawable balance.
//! No checkout or payout handler calls this module until the policy and
//! verified financial origin are independently approved by D/I.
use std::collections::HashSet;

const BOOK_WEIGHT_BPS: i128 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookWeighting {
    /// NEwBOO's documented split: equal whole-minor shares, residue to the
    /// last Book in canonical ID order. Select this rule explicitly.
    LegacyEqualLast,
    /// Explicit per-Book weights: largest fractional remainder, ties to
    /// the smallest Book ID. Not the old NEwBOO default.
    WeightedLargestRemainder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionError {
    UnconfirmedPolicy,
    InvalidCharge,
    InvalidBookSet,
    InvalidWeights,
    IncompleteShareSnapshot,
    Overflow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenParticipant {
    pub user_id: i64,
    pub percent: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenBook {
    pub book_id: i64,
    pub share_revision: i64,
    /// A number in 1..=10_000 only for WeightedLargestRemainder.
    pub weight_bps: Option<u16>,
    pub participants: Vec<FrozenParticipant>,
}

#[derive(Clone, Debug)]
pub struct AttributionInputs {
    pub payment_id: i64,
    pub term_id: i64,
    pub gross_minor: i64,
    pub currency: String,
    /// Versioned, explicitly approved by a trusted finance authority.
    pub policy_id: Option<String>,
    /// None means UNKNOWN, not a zero-percent fee.
    pub platform_fee_minor: Option<i64>,
    pub weighting: Option<BookWeighting>,
    /// The caller must validate the authentic per-term B-10 snapshot in PG.
    pub frozen_schedule_verified: bool,
    /// The allocation basis must be approved independently of B-10 shares.
    pub allocation_basis_confirmed: bool,
    pub books: Vec<FrozenBook>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParticipantAmount {
    pub user_id: i64,
    pub amount_minor: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookAmount {
    pub book_id: i64,
    pub share_revision: i64,
    pub amount_minor: i64,
    pub participants: Vec<ParticipantAmount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributionPreview {
    pub payment_id: i64,
    pub term_id: i64,
    pub currency: String,
    pub policy_id: String,
    pub gross_minor: i64,
    pub platform_fee_minor: i64,
    pub books: Vec<BookAmount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDraftLine {
    pub book_id: Option<i64>,
    pub recipient_id: Option<i64>,
    pub share_revision: Option<i64>,
    pub delta_minor: i64,
}

impl AttributionPreview {
    /// Includes zero-value participants for complete historical provenance.
    pub fn attributed_minor(&self) -> i128 {
        i128::from(self.platform_fee_minor)
            + self.books.iter().map(|b| {
                b.participants.iter().map(|p| i128::from(p.amount_minor)).sum::<i128>()
            }).sum::<i128>()
    }

    /// Mathematical negative mirror only. An authorized refund and a unique
    /// reversal posting still require a separate D PostgreSQL transaction.
    pub fn full_reversal_draft(&self) -> Vec<SignedDraftLine> {
        let mut lines = vec![SignedDraftLine {
            book_id: None,
            recipient_id: None,
            share_revision: None,
            delta_minor: -self.platform_fee_minor,
        }];
        for book in &self.books {
            for participant in &book.participants {
                lines.push(SignedDraftLine {
                    book_id: Some(book.book_id),
                    recipient_id: Some(participant.user_id),
                    share_revision: Some(book.share_revision),
                    delta_minor: -participant.amount_minor,
                });
            }
        }
        lines
    }
}

/// Integer largest-remainder allocation, deterministic even if the caller's
/// rows arrive in arbitrary order. All products are i128, never float.
fn largest_remainder(
    total_minor: i64,
    weights: &[(i64, i64)],
    denominator: i128,
) -> Result<Vec<(i64, i64)>, AttributionError> {
    if total_minor < 0
        || weights.is_empty()
        || weights.iter().any(|(id, weight)| *id <= 0 || *weight <= 0)
        || weights.iter().map(|(_, weight)| i128::from(*weight)).sum::<i128>() != denominator
    {
        return Err(AttributionError::InvalidWeights);
    }
    let mut shares: Vec<(i64, i64, i128)> = weights
        .iter()
        .map(|(id, weight)| {
            let product = i128::from(total_minor) * i128::from(*weight);
            Ok((
                *id,
                i64::try_from(product / denominator).map_err(|_| AttributionError::Overflow)?,
                product % denominator,
            ))
        })
        .collect::<Result<Vec<_>, AttributionError>>()?;
    let assigned: i128 = shares.iter().map(|(_, value, _)| i128::from(*value)).sum();
    let left = i128::from(total_minor) - assigned;
    if left < 0 || left > shares.len() as i128 {
        return Err(AttributionError::Overflow);
    }
    let mut rank: Vec<usize> = (0..shares.len()).collect();
    rank.sort_by(|a, b| {
        shares[*b].2.cmp(&shares[*a].2).then_with(|| shares[*a].0.cmp(&shares[*b].0))
    });
    for index in rank.into_iter().take(left as usize) {
        shares[index].1 = shares[index].1.checked_add(1).ok_or(AttributionError::Overflow)?;
    }
    shares.sort_by_key(|item| item.0);
    Ok(shares.into_iter().map(|(id, amount, _)| (id, amount)).collect())
}

/// Explicit finance inputs only. No money is posted, transferred, cleared or
/// made withdrawable. Snapshot authenticity cannot be proven by this function.
pub fn preview_layer_attribution(
    input: &AttributionInputs,
) -> Result<AttributionPreview, AttributionError> {
    let policy = input.policy_id.as_ref().filter(|s| {
        !s.is_empty() && s.len() <= 80 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }).ok_or(AttributionError::UnconfirmedPolicy)?;
    let fee = input.platform_fee_minor.ok_or(AttributionError::UnconfirmedPolicy)?;
    let weighting = input.weighting.ok_or(AttributionError::UnconfirmedPolicy)?;
    if !input.frozen_schedule_verified || !input.allocation_basis_confirmed {
        return Err(AttributionError::UnconfirmedPolicy);
    }
    if input.payment_id <= 0 || input.term_id <= 0 || input.gross_minor <= 0
        || input.currency != "RUB" || fee < 0 || fee > input.gross_minor
    {
        return Err(AttributionError::InvalidCharge);
    }
    if input.books.is_empty() {
        return Err(AttributionError::InvalidBookSet);
    }
    let mut seen_books = HashSet::new();
    for book in &input.books {
        if book.book_id <= 0 || book.share_revision <= 0 || !seen_books.insert(book.book_id) {
            return Err(AttributionError::InvalidBookSet);
        }
        let mut seen_people = HashSet::new();
        let mut sum = 0_i64;
        for participant in &book.participants {
            if participant.user_id <= 0
                || !(1..=100).contains(&participant.percent)
                || !seen_people.insert(participant.user_id)
            {
                return Err(AttributionError::IncompleteShareSnapshot);
            }
            sum += i64::from(participant.percent);
        }
        if sum != 100 || book.participants.is_empty() {
            return Err(AttributionError::IncompleteShareSnapshot);
        }
    }
    let pool = input.gross_minor - fee;
    let mut ordered: Vec<&FrozenBook> = input.books.iter().collect();
    ordered.sort_by_key(|b| b.book_id);
    let per_book = match weighting {
        BookWeighting::LegacyEqualLast => {
            if ordered.iter().any(|b| b.weight_bps.is_some()) {
                return Err(AttributionError::InvalidWeights);
            }
            let count = i64::try_from(ordered.len()).map_err(|_| AttributionError::Overflow)?;
            let unit = pool / count;
            let residue = pool % count;
            ordered.iter().enumerate().map(|(index, b)| {
                (b.book_id, unit + if index + 1 == ordered.len() { residue } else { 0 })
            }).collect::<Vec<_>>()
        }
        BookWeighting::WeightedLargestRemainder => {
            let weights = ordered.iter().map(|b| {
                b.weight_bps
                    .filter(|w| *w > 0)
                    .map(|weight| (b.book_id, i64::from(weight)))
                    .ok_or(AttributionError::InvalidWeights)
            }).collect::<Result<Vec<_>, _>>()?;
            largest_remainder(pool, &weights, BOOK_WEIGHT_BPS)?
        }
    };
    let mut books = Vec::with_capacity(ordered.len());
    for (book, (_, book_amount)) in ordered.into_iter().zip(per_book) {
        let weights: Vec<(i64, i64)> = book.participants.iter()
            .map(|p| (p.user_id, i64::from(p.percent))).collect();
        let members = largest_remainder(book_amount, &weights, 100)?;
        books.push(BookAmount {
            book_id: book.book_id,
            share_revision: book.share_revision,
            amount_minor: book_amount,
            participants: members.into_iter().map(|(user_id, amount_minor)| {
                ParticipantAmount { user_id, amount_minor }
            }).collect(),
        });
    }
    let result = AttributionPreview {
        payment_id: input.payment_id,
        term_id: input.term_id,
        currency: input.currency.clone(),
        policy_id: policy.clone(),
        gross_minor: input.gross_minor,
        platform_fee_minor: fee,
        books,
    };
    if result.attributed_minor() != i128::from(input.gross_minor) {
        return Err(AttributionError::Overflow);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
