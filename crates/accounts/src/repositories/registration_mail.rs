//! Durable A-owned signup evidence; no raw address or IP stored here.
use super::PgAccounts;
use sqlx::{Postgres, Transaction};
use tor_contracts::{
    auth::UserId,
    mail::{RegistrationMailEvidence, SignupMailSource},
};

pub(super) async fn capture(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    source: SignupMailSource,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO account_registration_mail_sources \
         (user_id,recipient_key,registration_ip_key,key_binding) VALUES($1,$2,$3,$4)",
    )
    .bind(user_id)
    .bind(source.recipient_key.as_slice())
    .bind(
        source
            .registration_ip_key
            .as_ref()
            .map(|key| key.as_slice()),
    )
    .bind(source.key_binding.as_slice())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) fn digest(bytes: Vec<u8>) -> Result<[u8; 32], sqlx::Error> {
    bytes
        .try_into()
        .map_err(|_| sqlx::Error::Protocol("invalid signup mail digest".to_owned()))
}

type SignupMailRow = (Vec<u8>, Option<Vec<u8>>, Vec<u8>);

impl PgAccounts {
    pub async fn registration_mail_source(
        &self,
        user_id: UserId,
    ) -> Result<Option<RegistrationMailEvidence>, sqlx::Error> {
        let row: Option<SignupMailRow> = sqlx::query_as(
            "SELECT recipient_key,registration_ip_key,key_binding \
             FROM account_registration_mail_sources \
             WHERE user_id=$1 AND created_at>clock_timestamp()-interval '7 days'",
        )
        .bind(user_id.get())
        .fetch_optional(self.pool())
        .await?;
        row.map(|(recipient, ip, binding)| {
            Ok(RegistrationMailEvidence {
                user_id,
                source: SignupMailSource {
                    recipient_key: digest(recipient)?,
                    registration_ip_key: ip.map(digest).transpose()?,
                    key_binding: digest(binding)?,
                },
            })
        })
        .transpose()
    }

    pub(super) async fn prune_registration_mail_sources(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH batch AS (SELECT user_id FROM account_registration_mail_sources \
             WHERE created_at<=clock_timestamp()-interval '7 days' \
             ORDER BY created_at,user_id LIMIT 1000 FOR UPDATE SKIP LOCKED) \
             DELETE FROM account_registration_mail_sources s USING batch b \
             WHERE s.user_id=b.user_id",
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
