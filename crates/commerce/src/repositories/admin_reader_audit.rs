//! S065: fail-closed, metadata-only evidence of privileged chapter delivery.
//! Never record body, object ID, signed URLs, DEKs, purchase IDs or sessions.
use sqlx::PgPool;
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

#[derive(Clone, Copy)]
pub enum AdminReadAction {
    UnsealedBody,
    SealedChapterGrant,
    SealedAssetGrant,
}

impl AdminReadAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnsealedBody => "unsealed_body",
            Self::SealedChapterGrant => "sealed_chapter_grant",
            Self::SealedAssetGrant => "sealed_asset_grant",
        }
    }
}

#[derive(Clone)]
pub struct PgAdminChapterAccessAudit {
    pool: PgPool,
}

impl PgAdminChapterAccessAudit {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The admin role and live session are locked for this transaction,
    /// preventing revocation from racing an audit INSERT. If no durable
    /// audit can commit, the caller MUST suppress the privileged response.
    pub async fn record(
        &self,
        digest: &[u8; 32],
        actor: PositiveId,
        chapter: PositiveId,
        version: PositiveId,
        action: AdminReadAction,
    ) -> Result<(), DeliveryError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let authorized: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM users u
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$1
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE u.id=$2 AND u.account_state='active'
             FOR SHARE OF u,s,r",
        )
        .bind(digest.as_slice())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        if authorized != Some(actor.get()) {
            return Err(DeliveryError::Denied);
        }
        sqlx::query(
            "INSERT INTO reader_admin_chapter_access_audit
             (admin_user_id,chapter_id,version_id,action)
             VALUES($1,$2,$3,$4)",
        )
        .bind(actor.get())
        .bind(chapter.get())
        .bind(version.get())
        .bind(action.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        tx.commit().await.map_err(|_| DeliveryError::Unavailable)?;
        Ok(())
    }
}
