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

    /// A historical unsealed edition is not a purchase or a public preview.
    /// Both the chapter/version relation and the live A administrator session
    /// must hold in the same query that identifies the private text.
    pub async fn unsealed_historical_version(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        version_id: PositiveId,
    ) -> Result<Option<ChapterVersionRef>, DeliveryError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT v.id
             FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN users u ON u.id=$3 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$4
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE v.chapter_id=$1 AND v.id=$2
               AND v.storage_object_id IS NULL
               AND length(v.body_markdown) BETWEEN 1 AND 200000
             LIMIT 1",
        )
        .bind(chapter_id.get())
        .bind(version_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(found
            .filter(|id| *id == version_id.get())
            .map(|_| ChapterVersionRef {
                chapter_id,
                version_id,
            }))
    }

    /// Bounded metadata-only history of unsealed editions for the private
    /// administrator reader. Sealed historic objects are not silently linked
    /// to a public or direct-buyer grant.
    pub async fn unsealed_history(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
    ) -> Result<Vec<(PositiveId, i32)>, DeliveryError> {
        let rows: Vec<(i64, i32)> = sqlx::query_as(
            "SELECT v.id,v.version_no
             FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN users u ON u.id=$2 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE v.chapter_id=$1 AND v.storage_object_id IS NULL
               AND length(v.body_markdown) BETWEEN 1 AND 200000
             ORDER BY v.version_no DESC,v.id DESC
             LIMIT 100",
        )
        .bind(chapter_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        rows.into_iter()
            .map(|(id, version_no)| {
                PositiveId::new(id)
                    .map(|version_id| (version_id, version_no))
                    .map_err(|_| DeliveryError::Unavailable)
            })
            .collect()
    }

    /// A historical sealed version is privileged inspection, never a buyer grant.
    /// Its exact chapter/version membership and the active administrator role
    /// are resolved by one SQL statement.
    pub async fn sealed_historical_version(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        version_id: PositiveId,
    ) -> Result<Option<ChapterVersionRef>, DeliveryError> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT v.id FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN storage_objects o ON o.id=v.storage_object_id
               AND o.state='attached' AND o.object_kind='chapter'
               AND o.codec='gzip'
             JOIN users u ON u.id=$3 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$4
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE v.chapter_id=$1 AND v.id=$2 AND c.access_level='paid'
             LIMIT 1",
        )
        .bind(chapter_id.get())
        .bind(version_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(id.filter(|found| *found == version_id.get()).map(|_| ChapterVersionRef {
            chapter_id,
            version_id,
        }))
    }

    /// Bounded private metadata: no S3 key, plaintext or signed URL.
    pub async fn sealed_history(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
    ) -> Result<Vec<(PositiveId, i32)>, DeliveryError> {
        let rows: Vec<(i64, i32)> = sqlx::query_as(
            "SELECT v.id,v.version_no FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN storage_objects o ON o.id=v.storage_object_id
               AND o.state='attached' AND o.object_kind='chapter'
               AND o.codec='gzip'
             JOIN users u ON u.id=$2 AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN account_roles r ON r.user_id=u.id
               AND r.role='administrator' AND r.is_active
             WHERE v.chapter_id=$1 AND c.access_level='paid'
             ORDER BY v.version_no DESC,v.id DESC LIMIT 100",
        )
        .bind(chapter_id.get())
        .bind(actor.user_id.get())
        .bind(self.session_digest.as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        rows.into_iter()
            .map(|(id, no)| PositiveId::new(id)
                .map(|version_id| (version_id, no))
                .map_err(|_| DeliveryError::Unavailable))
            .collect()
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
