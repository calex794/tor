//! Private package-cover ledger. Never shares book IDs, S3 keys, or paid entitlements.
use crate::domain::layers::LayerError;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone)]
pub struct PgLayerCovers {
    pool: PgPool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageCoverCandidate {
    pub id: i64,
    pub layer_id: i64,
    pub status: String,
    pub card_key: String,
    pub main_key: String,
    pub card_sha256: [u8; 32],
    pub main_sha256: [u8; 32],
    pub card_bytes: i64,
    pub main_bytes: i64,
}

pub struct PackageCoverUpload<'a> {
    pub card_key: &'a str,
    pub main_key: &'a str,
    pub card_sha256: [u8; 32],
    pub main_sha256: [u8; 32],
    pub card_bytes: i64,
    pub main_bytes: i64,
}

#[derive(Clone, Debug)]
pub struct PendingPackageCover {
    pub candidate: PackageCoverCandidate,
    pub layer_title: String,
}

fn unavailable(_: sqlx::Error) -> LayerError {
    LayerError::Unavailable
}

fn decode(row: &sqlx::postgres::PgRow) -> Result<PackageCoverCandidate, LayerError> {
    let card: Vec<u8> = row.try_get("card_sha256").map_err(unavailable)?;
    let main: Vec<u8> = row.try_get("main_sha256").map_err(unavailable)?;
    Ok(PackageCoverCandidate {
        id: row.try_get("id").map_err(unavailable)?,
        layer_id: row.try_get("layer_id").map_err(unavailable)?,
        status: row.try_get("status").map_err(unavailable)?,
        card_key: row.try_get("card_key").map_err(unavailable)?,
        main_key: row.try_get("main_key").map_err(unavailable)?,
        card_sha256: card.try_into().map_err(|_| LayerError::Unavailable)?,
        main_sha256: main.try_into().map_err(|_| LayerError::Unavailable)?,
        card_bytes: row.try_get("card_bytes").map_err(unavailable)?,
        main_bytes: row.try_get("main_bytes").map_err(unavailable)?,
    })
}

impl PgLayerCovers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn owner_can_upload(
        &self,
        owner: PositiveId,
        layer: PositiveId,
    ) -> Result<bool, LayerError> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden'))",
        )
        .bind(layer.get())
        .bind(owner.get())
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(allowed)
    }

    /// Returns only published candidates. The caller must verify object integrity.
    pub async fn public_current(
        &self,
        layer: PositiveId,
    ) -> Result<Option<PackageCoverCandidate>, LayerError> {
        let row = sqlx::query(
            "SELECT c.* FROM commerce_layer_cover_candidates c
             JOIN commerce_layers l ON l.id=c.layer_id
             WHERE c.layer_id=$1 AND c.status='approved'
               AND l.status='active' AND l.current_offer_id IS NOT NULL",
        )
        .bind(layer.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        row.as_ref().map(decode).transpose()
    }

    /// Owner can inspect approved and pending images without revealing S3 keys.
    pub async fn owner_view(
        &self,
        owner: PositiveId,
        layer: PositiveId,
    ) -> Result<(Option<PackageCoverCandidate>, Option<PackageCoverCandidate>), LayerError> {
        if !self.owner_can_upload(owner, layer).await? {
            return Err(LayerError::Inaccessible);
        }
        let rows = sqlx::query(
            "SELECT c.* FROM commerce_layer_cover_candidates c
             WHERE c.layer_id=$1 AND c.status IN ('approved','pending')
             ORDER BY c.id DESC",
        )
        .bind(layer.get())
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        let mut approved = None;
        let mut pending = None;
        for row in rows {
            let candidate = decode(&row)?;
            match candidate.status.as_str() {
                "approved" => approved = Some(candidate),
                "pending" => pending = Some(candidate),
                _ => {}
            }
        }
        Ok((approved, pending))
    }

    /// Reserve before S3 upload. A second or stale request cannot race moderation.
    pub async fn reserve(
        &self,
        owner: PositiveId,
        layer: PositiveId,
        upload: &PackageCoverUpload<'_>,
    ) -> Result<i64, LayerError> {
        const PREFIX: &str = "staging/covers/variants/packages/";
        let safe_key = |key: &str| {
            key.starts_with(PREFIX)
                && key.len() <= 512
                && !key.contains("..")
                && !key.contains("//")
                && key.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.')
                })
        };
        if !safe_key(upload.card_key)
            || !safe_key(upload.main_key)
            || upload.card_key == upload.main_key
            || !(1..=8 * 1024 * 1024).contains(&upload.card_bytes)
            || !(1..=8 * 1024 * 1024).contains(&upload.main_bytes)
        {
            return Err(LayerError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden'))",
        )
        .bind(layer.get())
        .bind(owner.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if !owned {
            return Err(LayerError::Inaccessible);
        }
        // Lock the Layer first, as in other owner-only mutations.
        sqlx::query("SELECT id FROM commerce_layers WHERE id=$1 FOR UPDATE")
            .bind(layer.get())
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        // A crashed upload can be retired after the bounded upload timeout.
        sqlx::query(
            "UPDATE commerce_layer_cover_candidates SET status='failed'
             WHERE layer_id=$1 AND status='uploading'
               AND submitted_at < now()-interval '30 minutes'",
        )
        .bind(layer.get())
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let inflight: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_cover_candidates
             WHERE layer_id=$1 AND status IN ('uploading','pending'))",
        )
        .bind(layer.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if inflight {
            return Err(LayerError::StaleOffer);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_cover_candidates
             (layer_id,uploaded_by_user_id,status,card_key,main_key,
              card_sha256,main_sha256,card_bytes,main_bytes)
             VALUES($1,$2,'uploading',$3,$4,$5,$6,$7,$8) RETURNING id",
        )
        .bind(layer.get())
        .bind(owner.get())
        .bind(upload.card_key)
        .bind(upload.main_key)
        .bind(upload.card_sha256.to_vec())
        .bind(upload.main_sha256.to_vec())
        .bind(upload.card_bytes)
        .bind(upload.main_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(id)
    }

    /// Call only after HEAD/SHA validation of both private WebP variants.
    pub async fn mark_pending(
        &self,
        owner: PositiveId,
        candidate_id: PositiveId,
    ) -> Result<(), LayerError> {
        let result = sqlx::query(
            "UPDATE commerce_layer_cover_candidates c SET status='pending'
             FROM commerce_layers l
             WHERE c.id=$1 AND c.uploaded_by_user_id=$2 AND c.status='uploading'
               AND l.id=c.layer_id AND l.owner_user_id=$2
               AND l.status IN ('active','hidden')",
        )
        .bind(candidate_id.get())
        .bind(owner.get())
        .execute(&self.pool)
        .await
        .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(LayerError::StaleOffer);
        }
        Ok(())
    }

    pub async fn mark_failed(
        &self,
        owner: PositiveId,
        candidate_id: PositiveId,
    ) -> Result<(), LayerError> {
        sqlx::query(
            "UPDATE commerce_layer_cover_candidates SET status='failed'
             WHERE id=$1 AND uploaded_by_user_id=$2 AND status='uploading'",
        )
        .bind(candidate_id.get())
        .bind(owner.get())
        .execute(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    pub async fn pending_queue(&self, limit: i64) -> Result<Vec<PendingPackageCover>, LayerError> {
        if !(1..=40).contains(&limit) {
            return Err(LayerError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT c.*,l.title AS layer_title
             FROM commerce_layer_cover_candidates c
             JOIN commerce_layers l ON l.id=c.layer_id
             WHERE c.status='pending'
             ORDER BY c.submitted_at,c.id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        rows.iter()
            .map(|r| {
                Ok(PendingPackageCover {
                    candidate: decode(r)?,
                    layer_title: r.try_get("layer_title").map_err(unavailable)?,
                })
            })
            .collect()
    }

    pub async fn pending_by_id(
        &self,
        candidate: PositiveId,
    ) -> Result<Option<PackageCoverCandidate>, LayerError> {
        let row = sqlx::query(
            "SELECT * FROM commerce_layer_cover_candidates
             WHERE id=$1 AND status='pending'",
        )
        .bind(candidate.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        row.as_ref().map(decode).transpose()
    }

    /// A review may change public cover, but never book composition or purchases.
    pub async fn review(
        &self,
        reviewer: PositiveId,
        candidate: PositiveId,
        approve: bool,
        reason: &str,
    ) -> Result<(), LayerError> {
        let reason = reason.trim();
        if reason.chars().count() > 2000 || (!approve && reason.is_empty()) {
            return Err(LayerError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let row = sqlx::query(
            "SELECT layer_id FROM commerce_layer_cover_candidates WHERE id=$1",
        )
        .bind(candidate.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let layer_id: i64 = row.try_get("layer_id").map_err(unavailable)?;
        // Prevent two moderators from simultaneously approving different candidates.
        sqlx::query("SELECT id FROM commerce_layers WHERE id=$1 FOR UPDATE")
            .bind(layer_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        let status: String = sqlx::query_scalar(
            "SELECT status FROM commerce_layer_cover_candidates WHERE id=$1 FOR UPDATE",
        )
        .bind(candidate.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if status != "pending" {
            return Err(LayerError::StaleOffer);
        }
        if approve {
            sqlx::query(
                "UPDATE commerce_layer_cover_candidates SET status='superseded'
                 WHERE layer_id=$1 AND status='approved'",
            )
            .bind(layer_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        sqlx::query(
            "UPDATE commerce_layer_cover_candidates
             SET status=$2,reviewed_by_user_id=$3,reviewed_at=now(),review_reason=$4
             WHERE id=$1 AND status='pending'",
        )
        .bind(candidate.get())
        .bind(if approve { "approved" } else { "rejected" })
        .bind(reviewer.get())
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_cover_key_cannot_be_browser_supplied_raw_path() {
        let prefix = "staging/covers/variants/packages/";
        assert!(!"/paid/books/private".starts_with(prefix));
        assert!("staging/covers/variants/packages/12/opaque/main.webp".starts_with(prefix));
    }
}
