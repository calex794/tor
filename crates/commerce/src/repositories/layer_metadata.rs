//! D-05 / S032: owner-only layer metadata CAS, not an offer/term change.
use super::layers::PgLayers;
use crate::domain::layers::LayerError;
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Only the owner may toggle active/hidden. A blocked layer is operator-owned.
    /// No version, offer, wallet or entitlement row is ever modified.
    pub async fn update_metadata_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_revision: i64,
        title: &str,
        description: &str,
        status: &str,
    ) -> Result<(), LayerError> {
        let title = title.trim();
        let description = description.trim();
        if expected_revision <= 0
            || title.is_empty()
            || title.chars().count() > 120
            || description.chars().count() > 2000
            || !matches!(status, "active" | "hidden")
        {
            return Err(LayerError::Invalid);
        }
        // One atomic statement is the compare-and-swap serialization point.
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE commerce_layers
             SET title=$4, description=$5, status=$6,
                 metadata_revision=metadata_revision+1
             WHERE id=$1 AND owner_user_id=$2
               AND metadata_revision=$3
               AND status IN ('active','hidden')
             RETURNING id",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(expected_revision)
        .bind(title)
        .bind(description)
        .bind(status)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if updated.is_some() {
            return Ok(());
        }
        let current: Option<String> = sqlx::query_scalar(
            "SELECT status FROM commerce_layers WHERE id=$1 AND owner_user_id=$2",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        match current.as_deref() {
            Some("active" | "hidden") => Err(LayerError::StaleOffer),
            _ => Err(LayerError::Inaccessible),
        }
    }
}
