//! One live book campaign, an active continuous E follow and exact kopecks.
use crate::domain::money::CheckoutError;
use sqlx::{Postgres, Row, Transaction};

/// V1 uses integer-kopek arithmetic with the discount rounded down.
pub(super) const DISCOUNT_RULE_VERSION: i16 = 1;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct DiscountApplied {
    pub campaign_id: i64,
    pub campaign_revision: i32,
    pub rule_version: i16,
    pub follow_period_id: i64,
    pub base_price_minor: i64,
    pub discount_minor: i64,
    pub paid_minor: i64,
    pub discount_percent: i16,
    pub min_follow_days: i16,
}

pub(super) async fn eligible_discount(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book_id: i64,
    owner: Option<i64>,
    base_price_minor: i64,
) -> Result<Option<DiscountApplied>, CheckoutError> {
    let Some(owner) = owner else {
        return Ok(None);
    };
    if actor <= 0 || book_id <= 0 || base_price_minor <= 0 || actor == owner {
        return Ok(None);
    }
    let campaign = sqlx::query(
        "SELECT id,revision,discount_percent,min_follow_days
         FROM author_discount_campaigns
         WHERE book_id=$1 AND owner_user_id=$2 AND status='active'
           AND starts_at<=now() AND ends_at>now()
         FOR SHARE",
    )
    .bind(book_id)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;
    let Some(campaign) = campaign else {
        return Ok(None);
    };
    let campaign_id: i64 = campaign
        .try_get("id")
        .map_err(|_| CheckoutError::Unavailable)?;
    let revision: i32 = campaign
        .try_get("revision")
        .map_err(|_| CheckoutError::Unavailable)?;
    let percent: i16 = campaign
        .try_get("discount_percent")
        .map_err(|_| CheckoutError::Unavailable)?;
    let days: i16 = campaign
        .try_get("min_follow_days")
        .map_err(|_| CheckoutError::Unavailable)?;

    // Lock the actual active period: unfollow cannot commit between this
    // eligibility decision and the wallet debit, even on another replica.
    let period = sqlx::query(
        "SELECT id, started_at <= now()-make_interval(days=>$3) AS mature
         FROM creator_follow_periods
         WHERE follower_user_id=$1 AND creator_user_id=$2 AND ended_at IS NULL
         FOR SHARE",
    )
    .bind(actor)
    .bind(owner)
    .bind(i32::from(days))
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;
    let Some(period) = period else {
        return Ok(None);
    };
    if !period
        .try_get::<bool, _>("mature")
        .map_err(|_| CheckoutError::Unavailable)?
    {
        return Ok(None);
    }

    // Exact integer calculation. percent<=90 ensures positive paid amount.
    // Small chapter prices with a zero-kopek rounding discount stay full price.
    let discount = i64::try_from(i128::from(base_price_minor) * i128::from(percent) / 100_i128)
        .map_err(|_| CheckoutError::Unavailable)?;
    if discount == 0 {
        return Ok(None);
    }
    let paid = base_price_minor
        .checked_sub(discount)
        .filter(|v| *v > 0)
        .ok_or(CheckoutError::Unavailable)?;
    Ok(Some(DiscountApplied {
        campaign_id,
        campaign_revision: revision,
        rule_version: DISCOUNT_RULE_VERSION,
        follow_period_id: period
            .try_get("id")
            .map_err(|_| CheckoutError::Unavailable)?,
        base_price_minor,
        discount_minor: discount,
        paid_minor: paid,
        discount_percent: percent,
        min_follow_days: days,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn exact_discount_example() {
        // 20% of 100 kopeks is 20 kopeks; all currency arithmetic uses ints.
        assert_eq!(i128::from(100) * i128::from(20) / 100, 20);
        assert_eq!(100 - 20, 80);
        assert_eq!(i128::from(1) * i128::from(20) / 100, 0);
    }
}
