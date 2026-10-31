//! D-08: deterministic, policy-driven chapter revenue allocation.
//!
//! The platform fee and the immutable policy version are explicit inputs.
//! No 20%/80% default is enabled here. Contributor percentages describe the
//! creator pool, not the gross sale. All arithmetic is in RUB kopecks.
//! A creator hold must use the exact refund deadline stored on the purchase;
//! this module never invents a separate "now + N days" maturity date.

use std::collections::HashSet;

use tor_contracts::paid_delivery::PositiveId;

use super::money::MoneyMinor;

const BASIS_POINTS: i128 = 10_000;
const HALF_BASIS: i128 = BASIS_POINTS / 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevenueAllocationError {
    InvalidPolicyVersion,
    InvalidPlatformFee,
    ZeroGrossAmount,
    EmptyShareSchedule,
    InvalidSharePercent,
    DuplicateRecipient,
    SharesMustEqualOneHundred,
    Overflow,
    MissingRefundDeadline,
    InvalidRefundDeadline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevenueAllocationPolicy {
    version: String,
    platform_fee_bps: u16,
}

impl RevenueAllocationPolicy {
    /// A policy is always supplied by an approved, versioned source.
    /// There is deliberately no default policy or implicit fee rate.
    pub fn new(
        version: impl Into<String>,
        platform_fee_bps: u16,
    ) -> Result<Self, RevenueAllocationError> {
        let version = version.into();
        if version.is_empty()
            || version.len() > 80
            || version.trim() != version
            || version.chars().any(char::is_whitespace)
        {
            return Err(RevenueAllocationError::InvalidPolicyVersion);
        }
        if i128::from(platform_fee_bps) > BASIS_POINTS {
            return Err(RevenueAllocationError::InvalidPlatformFee);
        }
        Ok(Self {
            version,
            platform_fee_bps,
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn platform_fee_bps(&self) -> u16 {
        self.platform_fee_bps
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevenueShare {
    pub recipient_id: PositiveId,
    pub share_percent: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatorAllocation {
    pub recipient_id: PositiveId,
    pub share_percent: u8,
    pub amount_minor: MoneyMinor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchaseAllocation {
    pub policy_version: String,
    pub gross_minor: MoneyMinor,
    pub platform_fee_minor: MoneyMinor,
    pub creator_pool_minor: MoneyMinor,
    pub creators: Vec<CreatorAllocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatorEarningHold {
    pub purchase_id: PositiveId,
    pub recipient_id: PositiveId,
    pub amount_minor: MoneyMinor,
    /// Unix seconds copied from the purchase's immutable refund deadline.
    pub available_at_unix: i64,
}

#[derive(Clone, Copy)]
struct RemainderRank {
    index: usize,
    remainder: i128,
    recipient_id: i64,
}

/// Calculate the exact gross → platform fee + creator pool split.
///
/// Platform fee uses ROUND_HALF_UP at the kopeck boundary. The creator pool
/// is distributed by largest remainder; equal remainders are awarded in
/// ascending recipient ID order, independent of database row order.
pub fn calculate_purchase_allocation(
    gross_minor: MoneyMinor,
    policy: &RevenueAllocationPolicy,
    shares: &[RevenueShare],
) -> Result<PurchaseAllocation, RevenueAllocationError> {
    if gross_minor.get() <= 0 {
        return Err(RevenueAllocationError::ZeroGrossAmount);
    }
    if shares.is_empty() {
        return Err(RevenueAllocationError::EmptyShareSchedule);
    }

    let mut seen = HashSet::with_capacity(shares.len());
    let mut total_share = 0_u16;
    for share in shares {
        if !(1..=100).contains(&share.share_percent) {
            return Err(RevenueAllocationError::InvalidSharePercent);
        }
        if !seen.insert(share.recipient_id.get()) {
            return Err(RevenueAllocationError::DuplicateRecipient);
        }
        total_share = total_share
            .checked_add(u16::from(share.share_percent))
            .ok_or(RevenueAllocationError::Overflow)?;
    }
    if total_share != 100 {
        return Err(RevenueAllocationError::SharesMustEqualOneHundred);
    }

    let gross = i128::from(gross_minor.get());
    let fee_numerator = gross * i128::from(policy.platform_fee_bps);
    let fee_minor = (fee_numerator + HALF_BASIS) / BASIS_POINTS;
    let fee_minor = i64::try_from(fee_minor).map_err(|_| RevenueAllocationError::Overflow)?;
    let creator_pool = gross_minor
        .get()
        .checked_sub(fee_minor)
        .ok_or(RevenueAllocationError::Overflow)?;
    let creator_pool_i128 = i128::from(creator_pool);

    let mut amounts = Vec::with_capacity(shares.len());
    let mut ranking = Vec::with_capacity(shares.len());
    let mut distributed = 0_i128;

    for (index, share) in shares.iter().enumerate() {
        let product = creator_pool_i128 * i128::from(share.share_percent);
        let base = product / 100;
        let remainder = product % 100;
        let base = i64::try_from(base).map_err(|_| RevenueAllocationError::Overflow)?;
        distributed += i128::from(base);
        amounts.push(base);
        ranking.push(RemainderRank {
            index,
            remainder,
            recipient_id: share.recipient_id.get(),
        });
    }

    let remaining = creator_pool_i128 - distributed;
    let remaining = usize::try_from(remaining).map_err(|_| RevenueAllocationError::Overflow)?;
    ranking.sort_by(|left, right| {
        right
            .remainder
            .cmp(&left.remainder)
            .then_with(|| left.recipient_id.cmp(&right.recipient_id))
    });
    if remaining > ranking.len() {
        return Err(RevenueAllocationError::Overflow);
    }
    for rank in ranking.into_iter().take(remaining) {
        amounts[rank.index] = amounts[rank.index]
            .checked_add(1)
            .ok_or(RevenueAllocationError::Overflow)?;
    }

    let mut creators = Vec::with_capacity(shares.len());
    let mut allocated = 0_i64;
    for (share, amount) in shares.iter().zip(amounts) {
        allocated = allocated
            .checked_add(amount)
            .ok_or(RevenueAllocationError::Overflow)?;
        creators.push(CreatorAllocation {
            recipient_id: share.recipient_id,
            share_percent: share.share_percent,
            amount_minor: MoneyMinor::new(amount)
                .map_err(|_| RevenueAllocationError::Overflow)?,
        });
    }
    creators.sort_by_key(|line| line.recipient_id.get());

    if allocated != creator_pool {
        return Err(RevenueAllocationError::Overflow);
    }
    let platform_fee_minor =
        MoneyMinor::new(fee_minor).map_err(|_| RevenueAllocationError::Overflow)?;
    let creator_pool_minor =
        MoneyMinor::new(creator_pool).map_err(|_| RevenueAllocationError::Overflow)?;
    if platform_fee_minor
        .checked_add(creator_pool_minor)
        .map_err(|_| RevenueAllocationError::Overflow)?
        != gross_minor
    {
        return Err(RevenueAllocationError::Overflow);
    }

    Ok(PurchaseAllocation {
        policy_version: policy.version.clone(),
        gross_minor,
        platform_fee_minor,
        creator_pool_minor,
        creators,
    })
}

impl PurchaseAllocation {
    /// Build pending creator holds from the *purchase's* persisted refund
    /// deadline. Missing/invalid deadline is a hard failure; never invent a
    /// second maturity clock after the purchase has committed.
    pub fn creator_holds(
        &self,
        purchase_id: PositiveId,
        refund_deadline_at_unix: Option<i64>,
    ) -> Result<Vec<CreatorEarningHold>, RevenueAllocationError> {
        let has_positive_earning = self.creators.iter().any(|line| line.amount_minor.get() > 0);
        if !has_positive_earning {
            return Ok(Vec::new());
        }
        let available_at_unix = refund_deadline_at_unix
            .ok_or(RevenueAllocationError::MissingRefundDeadline)?;
        if available_at_unix <= 0 {
            return Err(RevenueAllocationError::InvalidRefundDeadline);
        }
        Ok(self
            .creators
            .iter()
            .filter(|line| line.amount_minor.get() > 0)
            .map(|line| CreatorEarningHold {
                purchase_id,
                recipient_id: line.recipient_id,
                amount_minor: line.amount_minor,
                available_at_unix,
            })
            .collect())
    }
}
