//! D-05: owner-only book picker with the same eligibility as add_book().
//! The picker is not a grant to write: add_book() takes the Book lock and
//! rechecks every condition, including races with team membership changes.
use super::layers::PgLayers;
use crate::domain::layers::{
    LayerBookCandidate, LayerBookCandidatePage, LayerCurrentBookPage, LayerError, OwnedLayerSummary,
};
use sqlx::Row;
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Private author navigation, not a public Layer catalogue.
    /// Bounded results and actor ownership are enforced in SQL.
    pub async fn owned_layers(
        &self,
        actor: PositiveId,
        limit: u16,
    ) -> Result<Vec<OwnedLayerSummary>, LayerError> {
        if !(1..=100).contains(&limit) {
            return Err(LayerError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT id,title FROM commerce_layers
             WHERE owner_user_id=$1 AND status='active'
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor.get())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(OwnedLayerSummary {
                    layer_id: row.try_get("id").map_err(|_| LayerError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
                })
            })
            .collect()
    }

    /// Read only the owner's immutable current version. This is advisory:
    /// remove_book_if_current() revalidates the version under the Layer lock.
    pub async fn current_books(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        limit: u16,
    ) -> Result<LayerCurrentBookPage, LayerError> {
        if !(1..=50).contains(&limit) {
            return Err(LayerError::Invalid);
        }
        let version: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active'",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let records = sqlx::query(
            "SELECT vb.book_id,b.title FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1 AND vb.book_id>$2
             ORDER BY vb.book_id LIMIT $3",
        )
        .bind(version)
        .bind(after.map_or(0, PositiveId::get))
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let has_more = records.len() > usize::from(limit);
        let mut books = Vec::with_capacity(records.len().min(usize::from(limit)));
        for record in records.into_iter().take(usize::from(limit)) {
            books.push(LayerBookCandidate {
                book_id: record
                    .try_get("book_id")
                    .map_err(|_| LayerError::Unavailable)?,
                title: record
                    .try_get("title")
                    .map_err(|_| LayerError::Unavailable)?,
            });
        }
        Ok(LayerCurrentBookPage {
            current_version_id: version,
            next_after: if has_more {
                books.last().map(|book| book.book_id)
            } else {
                None
            },
            books,
        })
    }

    /// Keyset-paginated, bounded books that this actor may add to a Layer now.
    /// A collaborator with even a zero-percent share makes a Book ineligible.
    /// Historical (removed) memberships are not considered active team seats.
    pub async fn addable_books(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        limit: u16,
    ) -> Result<LayerBookCandidatePage, LayerError> {
        if !(1..=50).contains(&limit) {
            return Err(LayerError::Invalid);
        }
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active'",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let current = current.ok_or(LayerError::Inaccessible)?;

        // This must match PgLayers::add_book's server-side check exactly.
        // No Book lock here: a picker GET is advisory and can become stale.
        let results = sqlx::query(
            "SELECT b.id,b.title FROM books b
             WHERE b.owner_user_id=$1
               AND b.id>$2
               AND NOT b.admin_hold
               AND NOT b.is_deleted
               AND EXISTS(
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$1
                   AND m.is_active AND m.role='owner'
               )
               AND NOT EXISTS(
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.is_active
                   AND (m.user_id<>$1 OR m.role<>'owner')
               )
               AND NOT EXISTS(
                 SELECT 1 FROM commerce_layer_version_books vb
                 WHERE vb.layer_version_id=$3 AND vb.book_id=b.id
               )
             ORDER BY b.id LIMIT $4",
        )
        .bind(actor.get())
        .bind(after.map_or(0, PositiveId::get))
        .bind(current)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;

        let has_more = results.len() > usize::from(limit);
        let mut books = Vec::with_capacity(results.len().min(usize::from(limit)));
        for record in results.into_iter().take(usize::from(limit)) {
            books.push(LayerBookCandidate {
                book_id: record.try_get("id").map_err(|_| LayerError::Unavailable)?,
                title: record
                    .try_get("title")
                    .map_err(|_| LayerError::Unavailable)?,
            });
        }
        Ok(LayerBookCandidatePage {
            next_after: if has_more {
                books.last().map(|book| book.book_id)
            } else {
                None
            },
            books,
        })
    }
}
