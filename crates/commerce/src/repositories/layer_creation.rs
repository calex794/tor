//! D-05: atomic, owner-scoped creation of an empty Layer container.
//! No checkout, author revenue, books or paid terms are created here.
use super::layers::PgLayers;
use crate::domain::{
    layers::{LayerCreateReceipt, LayerError},
    money::IdempotencyKey,
};
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Replaying one authenticated command yields exactly the original Layer.
    /// Changing the title under the same key is an explicit replay conflict.
    /// The unique owner/key constraint arbitrates concurrent requests in PG.
    pub async fn create_layer_once(
        &self,
        actor: PositiveId,
        title: &str,
        key: &IdempotencyKey,
    ) -> Result<LayerCreateReceipt, LayerError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let created: Option<i64> = sqlx::query_scalar(
            "INSERT INTO commerce_layers(owner_user_id,title,create_command_key)
             VALUES($1,$2,$3)
             ON CONFLICT (owner_user_id,create_command_key) DO NOTHING
             RETURNING id",
        )
        .bind(actor.get())
        .bind(title)
        .bind(key.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if let Some(layer_id) = created {
            let version: i64 = sqlx::query_scalar(
                "INSERT INTO commerce_layer_versions(layer_id,version_no)
                 VALUES($1,1) RETURNING id",
            )
            .bind(layer_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
            sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
                .bind(layer_id)
                .bind(version)
                .execute(&mut *tx)
                .await
                .map_err(|_| LayerError::Unavailable)?;
            tx.commit().await.map_err(|_| LayerError::Unavailable)?;
            return Ok(LayerCreateReceipt {
                layer_id,
                replayed: false,
            });
        }

        // The conflicting INSERT waits on any concurrent winner. This SELECT
        // sees its committed row; an unfinished/inconsistent row fails closed.
        let row: Option<(i64, String, Option<i64>)> = sqlx::query_as(
            "SELECT id,title,current_version_id FROM commerce_layers
             WHERE owner_user_id=$1 AND create_command_key=$2",
        )
        .bind(actor.get())
        .bind(key.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let Some((layer_id, saved_title, Some(_))) = row else {
            return Err(LayerError::Unavailable);
        };
        if saved_title != title {
            return Err(LayerError::ReplayConflict);
        }
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(LayerCreateReceipt {
            layer_id,
            replayed: true,
        })
    }
}
