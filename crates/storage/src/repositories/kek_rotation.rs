//! Operator-only S3-34 envelope rewrap in a PostgreSQL transaction.
//! No HTTP route or worker invokes this; never rewrite the S3 ciphertext.
use crate::services::crypto::{rewrap_registered_v1_to_v2, ContentKek};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::ObjectId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotationError {
    NotOperator,
    Unavailable,
    KeyRejected,
    InvalidRecord,
    Conflict,
}

pub struct PgKekRotation {
    pool: PgPool,
}

impl PgKekRotation {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Cryptographically authenticate the old DEK wrapper and commit only its
    /// replacement under a new KEK. A row lock plus exact old-value CAS makes
    /// repeated calls safe after crashes and prevents lost concurrent updates.
    /// Requires a verified PG+S3+KEK backup before operator invocation.
    pub async fn rewrap_one_v1_to_v2(
        &self,
        object: &ObjectId,
        v1: &ContentKek,
        v2: &ContentKek,
    ) -> Result<bool, RotationError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RotationError::Unavailable)?;
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| RotationError::Unavailable)?;
        if role != "tor_migrator" {
            return Err(RotationError::NotOperator);
        }
        let row = sqlx::query(
            "SELECT encrypted_dek,wrap_nonce FROM storage_objects
             WHERE id=$1::uuid AND state='attached' AND object_kind='chapter'
               AND kek_id='content-kek-v1' FOR UPDATE",
        )
        .bind(object.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RotationError::Unavailable)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let wrapped: Vec<u8> = row
            .try_get("encrypted_dek")
            .map_err(|_| RotationError::InvalidRecord)?;
        let nonce: Vec<u8> = row
            .try_get("wrap_nonce")
            .map_err(|_| RotationError::InvalidRecord)?;
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| RotationError::InvalidRecord)?;
        let new = rewrap_registered_v1_to_v2(object, &wrapped, &nonce, v1, v2)
            .map_err(|_| RotationError::KeyRejected)?;
        let changed: bool =
            sqlx::query_scalar("SELECT rewrap_attached_storage_v1_to_v2($1::uuid,$2,$3,$4,$5)")
                .bind(object.as_str())
                .bind(&wrapped)
                .bind(nonce.as_slice())
                .bind(&new.wrapped_dek)
                .bind(new.wrap_nonce.as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| RotationError::Unavailable)?;
        if !changed {
            return Err(RotationError::Conflict);
        }
        tx.commit().await.map_err(|_| RotationError::Unavailable)?;
        Ok(true)
    }

    /// A V1 secret may not be retired while ANY pending or attached object
    /// still references it (not just the currently published version).
    pub async fn count_v1_references(&self) -> Result<i64, RotationError> {
        sqlx::query_scalar("SELECT count(*) FROM storage_objects WHERE kek_id='content-kek-v1'")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| RotationError::Unavailable)
    }
}
