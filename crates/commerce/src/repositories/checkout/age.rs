//! S020/D: a new paid purchase must be readable under the buyer's
//! currently attested age tier. Existing purchases are never revoked here.
use crate::domain::money::CheckoutError;
use sqlx::{Postgres, Transaction};

/// Call after locking the exact public Book and BEFORE locking/debiting the
/// wallet. Lock order: Book SHARE -> buyer SHARE -> active pro grant SHARE ->
/// wallet UPDATE. A competing age revocation must either precede this read
/// (deny with no debit) or wait until the purchase has committed.
pub(super) async fn require_purchase_tier(
    tx: &mut Transaction<'_, Postgres>,
    buyer: i64,
    tier: &str,
) -> Result<(), CheckoutError> {
    let adult_enabled = sqlx::query_scalar::<_, bool>(
        "SELECT adult_enabled FROM users
         WHERE id=$1 AND account_state='active' FOR SHARE",
    )
    .bind(buyer)
    .fetch_optional(&mut **tx)
    .await
    .map_err(super::unavailable)?
    .ok_or(CheckoutError::Inaccessible)?;

    match tier {
        "basic" => Ok(()),
        "adult" if adult_enabled => Ok(()),
        "pro_adult" => {
            // Mirror the canonical tor_purchase_version_readable predicate.
            // Lock the matching row: without it, revocation may commit between
            // a stale EXISTS snapshot and the financial transaction's COMMIT.
            let active = sqlx::query_scalar::<_, bool>(
                "SELECT TRUE FROM user_level_grants
                 WHERE user_id=$1 AND level='pro_adult' AND is_active
                 LIMIT 1 FOR SHARE",
            )
            .bind(buyer)
            .fetch_optional(&mut **tx)
            .await
            .map_err(super::unavailable)?;
            if active.is_some() {
                Ok(())
            } else {
                Err(CheckoutError::Inaccessible)
            }
        }
        _ => Err(CheckoutError::Inaccessible),
    }
}
