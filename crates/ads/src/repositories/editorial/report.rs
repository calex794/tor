use super::{commands, limits, PgEditorial};
use crate::{domain::editorial::*, services::editorial::EditorialStore};
use async_trait::async_trait;
use sqlx::Row;
use tor_contracts::exposure::PromotionSlot;

#[async_trait]
impl EditorialStore for PgEditorial {
    async fn apply(
        &self,
        administrator: &Administrator,
        key: &str,
        command: &NormalizedCommand,
    ) -> Result<Receipt, EditorialError> {
        commands::apply(self, administrator, key, command).await
    }
    async fn campaigns(&self, after: Option<i64>) -> Result<CampaignPage, EditorialError> {
        if after.is_some_and(|id| id <= 0) {
            return Err(EditorialError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, true).await?;
        let rows=sqlx::query("SELECT c.id,c.book_id,b.title,c.slot,c.status,
          to_char(c.starts_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS starts_at,
          to_char(c.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS ends_at,
          c.priority,c.revision,EXISTS(SELECT 1 FROM rec_materializable_books p WHERE p.id=c.book_id) AS discoverable
          FROM editorial_campaigns c JOIN books b ON b.id=c.book_id
          WHERE ($1::bigint IS NULL OR c.id<$1) ORDER BY c.id DESC LIMIT 51")
            .bind(after).fetch_all(&mut *tx).await?;
        let mut items = rows
            .into_iter()
            .map(|row| {
                Ok(Campaign {
                    id: row.try_get("id")?,
                    book_id: row.try_get("book_id")?,
                    title: row.try_get("title")?,
                    slot: row
                        .try_get::<String, _>("slot")?
                        .parse()
                        .map_err(|_| EditorialError::Unavailable)?,
                    status: row.try_get("status")?,
                    starts_at: row.try_get("starts_at")?,
                    ends_at: row.try_get("ends_at")?,
                    priority: row.try_get("priority")?,
                    revision: row.try_get("revision")?,
                    discoverable: row.try_get("discoverable")?,
                })
            })
            .collect::<Result<Vec<_>, EditorialError>>()?;
        let next_cursor = if items.len() > 50 {
            items.truncate(50);
            items.last().map(|c| c.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(CampaignPage { items, next_cursor })
    }
    async fn cards(
        &self,
        viewer: Option<&Viewer>,
        slot: PromotionSlot,
        host: Option<i64>,
    ) -> Result<Vec<EditorialCard>, EditorialError> {
        self.live_cards(viewer, slot, host).await
    }
}
