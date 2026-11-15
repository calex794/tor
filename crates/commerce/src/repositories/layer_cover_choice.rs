//! Safe package cover choice: reuse only an already attached public book cover.
use super::layers::PgLayers;
use crate::domain::layers::LayerError;
use sqlx::Row;
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Cover metadata is separate from immutable version composition and money.
    /// A Book cover can be chosen only when the Book is in the current version,
    /// public and attached to a verified, approved card variant.
    pub async fn choose_package_cover(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
        expected_revision: i64,
    ) -> Result<(), LayerError> {
        if expected_revision <= 0 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let row = sqlx::query(
            "SELECT current_version_id,metadata_revision FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden')
             FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let revision: i64 = row
            .try_get("metadata_revision")
            .map_err(|_| LayerError::Unavailable)?;
        if revision != expected_revision {
            return Err(LayerError::StaleOffer);
        }
        let version: i64 = row
            .try_get("current_version_id")
            .map_err(|_| LayerError::Unavailable)?;
        let approved: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM commerce_layer_version_books vb
                JOIN books b ON b.id=vb.book_id
                WHERE vb.layer_version_id=$1 AND b.id=$2
                  AND b.owner_user_id=$3
                  AND b.visibility='public' AND b.visibility_status='active'
                  AND NOT b.is_deleted AND NOT b.admin_hold
                  AND EXISTS(
                      SELECT 1 FROM book_cover_slots s
                      JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
                      JOIN cover_variants v ON v.job_id=j.id
                            AND v.variant_name='card'
                      WHERE s.book_id=b.id AND j.status='ready'
                  )
            )",
        )
        .bind(version)
        .bind(book.get())
        .bind(actor.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if !approved {
            return Err(LayerError::Inaccessible);
        }
        sqlx::query(
            "UPDATE commerce_layers
             SET cover_book_id=$3,metadata_revision=metadata_revision+1
             WHERE id=$1 AND owner_user_id=$2",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(book.get())
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)
    }
}
