//! Current account-owned mailbox; never expose it in public responses or logs.
use super::PgAccounts;
use crate::domain::email::delivery_address;
use tor_contracts::auth::UserId;

impl PgAccounts {
    /// An active account and canonical syntax are necessary, not proof of
    /// mailbox ownership. The caller must enforce purpose-specific eligibility.
    pub async fn mail_delivery_address(
        &self,
        user_id: UserId,
    ) -> Result<Option<String>, sqlx::Error> {
        let address: Option<String> = sqlx::query_scalar(
            "SELECT email_normalized FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(user_id.get())
        .fetch_optional(self.pool())
        .await?;
        Ok(address.and_then(|address| delivery_address(&address).ok()))
    }
}
