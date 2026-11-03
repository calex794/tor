//! C-12 / S065: an active site administrator may inspect a sealed paid edition
//! without manufacturing a purchase. This is a reading capability, never
//! checkout, refund, wallet ownership or a generic direct-buyer entitlement.
use async_trait::async_trait;
use sqlx::PgPool;
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionRef, DeliveryError, DenialReason, EntitlementDecision, EntitlementPort,
    PositiveId,
};

/// Bound to the authenticated A session, not to an untrusted role claim.
pub struct PgAdministratorReaderEntitlements {
    pool: PgPool,
    session_digest: [u8; 32],
}

impl PgAdministratorReaderEntitlements {
    pub fn new(pool: PgPool, session_digest: [u8; 32]) -> Self {
        Self {
            pool,
            session_digest,
        }
    }

    /// Resolve only the current working draft or current published plaintext
    /// edition. A historical, sealed or browser-selected version is not
    /// implicitly authorized by this endpoint. A is checked in the same SQL.
    pub async fn unsealed_version(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
    ) -> Result<Option<ChapterVersionRef>, DeliveryError> {
        let version: Option<i64> = sqlx::query_scalar(
            "SELECT v.id
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
             LEFT JOIN chapter_publications cp ON cp.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id
               AND v.id=COALESCE(d.version_id,cp.version_id)
             JOIN users u ON u.id=$2 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE c.id=$1 AND v.storage_object_id IS NULL
               AND length(v.body_markdown) BETWEEN 1 AND 200000
             LIMIT 1",
        )
        .bind(chapter_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(version
            .and_then(|id| PositiveId::new(id).ok())
            .map(|version_id| ChapterVersionRef {
                chapter_id,
                version_id,
            }))
    }
}

#[async_trait]
impl EntitlementPort for PgAdministratorReaderEntitlements {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        // Admin inspection has no purchase identity, direct refund or Layer
        // authority. Reject even an otherwise valid foreign purchase_id.
        if purchase_id.is_some() {
            return Ok(EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased,
            });
        }
        // Authoritative A session + active role is re-read on EVERY call.
        // C invokes this before and after G signs, so a revoked role cannot
        // authorize a new bearer grant. No trusted choice of V1/V2 comes
        // from the browser. B/G must still verify the attached ciphertext.
        //
        // NEwBOO's canonical reader grants the current chapter version,
        // not the newest historical S3 object. In TOR the current working
        // pointer takes precedence over the published one. An orphaned sealed
        // version without either current pointer must never become readable.
        let version: Option<i64> = sqlx::query_scalar(
            "SELECT v.id
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
             LEFT JOIN chapter_publications cp ON cp.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id
               AND v.id=COALESCE(d.version_id,cp.version_id)
             JOIN storage_objects o ON o.id=v.storage_object_id
               AND o.state='attached' AND o.object_kind='chapter'
               AND o.codec='gzip'
             JOIN users u ON u.id=$2 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE c.id=$1 AND c.access_level='paid'
             LIMIT 1",
        )
        .bind(chapter_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;

        match version.and_then(|version| PositiveId::new(version).ok()) {
            Some(version_id) => Ok(EntitlementDecision::Allowed {
                pinned: ChapterVersionRef {
                    chapter_id,
                    version_id,
                },
            }),
            None => Ok(EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased,
            }),
        }
    }
}
