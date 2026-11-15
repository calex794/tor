//! Owner-only persisted package selection scratch state. Never a paid grant.
use super::{layer_composition::LayerBookTilePage, layers::PgLayers};
use crate::domain::layers::LayerError;
use sqlx::Row;
use std::collections::BTreeSet;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerSelectionDraft {
    pub base_version_id: i64,
    /// 0 is the initial missing row; positive revisions are persisted.
    pub revision: i64,
    pub additions: Vec<i64>,
    pub removals: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct LayerStagePage {
    pub version: PositiveId,
    pub revision: i64,
    pub after: Option<PositiveId>,
    pub query: String,
    /// All book IDs the client actually saw, in server keyset order.
    pub visible: Vec<i64>,
    /// Only checked books from that page, never full package composition.
    pub selected: Vec<i64>,
}

fn ids_valid(ids: &[i64]) -> bool {
    ids.len() <= 100
        && ids.iter().all(|id| *id > 0)
        && ids.iter().copied().collect::<BTreeSet<_>>().len() == ids.len()
}

impl PgLayers {
    /// Read a current, owned draft; reject stale drafts instead of silently
    /// reusing changes relative to a new paid package version.
    pub async fn selection_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<LayerSelectionDraft, LayerError> {
        let row = sqlx::query(
            "SELECT l.current_version_id,d.base_version_id,d.revision,
                    d.added_book_ids,d.removed_book_ids
             FROM commerce_layers l
             LEFT JOIN commerce_layer_selection_drafts d
               ON d.layer_id=l.id AND d.owner_user_id=l.owner_user_id
             WHERE l.id=$1 AND l.owner_user_id=$2
               AND l.status IN ('active','hidden')",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let current: i64 = row
            .try_get("current_version_id")
            .map_err(|_| LayerError::Unavailable)?;
        let stored: Option<i64> = row
            .try_get("base_version_id")
            .map_err(|_| LayerError::Unavailable)?;
        // Preserve a stale draft for explicit user review/discard. Never
        // silently rebase pending changes onto another immutable version.
        Ok(LayerSelectionDraft {
            base_version_id: stored.unwrap_or(current),
            revision: row
                .try_get::<Option<i64>, _>("revision")
                .map_err(|_| LayerError::Unavailable)?
                .unwrap_or(0),
            additions: row
                .try_get::<Option<Vec<i64>>, _>("added_book_ids")
                .map_err(|_| LayerError::Unavailable)?
                .unwrap_or_default(),
            removals: row
                .try_get::<Option<Vec<i64>>, _>("removed_book_ids")
                .map_err(|_| LayerError::Unavailable)?
                .unwrap_or_default(),
        })
    }

    /// Store one page worth of checks/unchecks without writing paid versions.
    /// The server recalculates exactly which candidates appeared in the grid.
    pub async fn stage_selection_page(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        command: &LayerStagePage,
    ) -> Result<(LayerSelectionDraft, LayerBookTilePage), LayerError> {
        if !ids_valid(&command.visible)
            || !ids_valid(&command.selected)
            || command.revision < 0
            || command.query.chars().count() > 100
        {
            return Err(LayerError::Invalid);
        }
        let visible: BTreeSet<i64> = command.visible.iter().copied().collect();
        if !command.selected.iter().all(|id| visible.contains(id)) {
            return Err(LayerError::Invalid);
        }
        let page = self
            .composition_tiles(actor, layer, command.after, &command.query, 50)
            .await?;
        if page.books.iter().map(|book| book.id).collect::<Vec<_>>() != command.visible {
            return Err(LayerError::StaleOffer);
        }
        let selected: BTreeSet<i64> = command.selected.iter().copied().collect();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        if current != command.version.get() {
            return Err(LayerError::StaleOffer);
        }
        let draft = sqlx::query(
            "SELECT base_version_id,revision,added_book_ids,removed_book_ids
             FROM commerce_layer_selection_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let (old_revision, mut additions, mut removals) = if let Some(row) = draft {
            let version: i64 = row
                .try_get("base_version_id")
                .map_err(|_| LayerError::Unavailable)?;
            if version != current {
                return Err(LayerError::StaleOffer);
            }
            (
                row.try_get::<i64, _>("revision")
                    .map_err(|_| LayerError::Unavailable)?,
                row.try_get::<Vec<i64>, _>("added_book_ids")
                    .map_err(|_| LayerError::Unavailable)?
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                row.try_get::<Vec<i64>, _>("removed_book_ids")
                    .map_err(|_| LayerError::Unavailable)?
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
            )
        } else {
            (0_i64, BTreeSet::new(), BTreeSet::new())
        };
        if command.revision != old_revision {
            return Err(LayerError::StaleOffer);
        }
        for book in &page.books {
            let checked = selected.contains(&book.id);
            // A checked member stays in the unchanged base version; an unchecked
            // member is a staged removal. Conversely for a new candidate.
            if checked && !book.included {
                additions.insert(book.id);
            } else {
                additions.remove(&book.id);
            }
            if !checked && book.included {
                removals.insert(book.id);
            } else {
                removals.remove(&book.id);
            }
        }
        if additions.len() + removals.len() > 100 {
            return Err(LayerError::Invalid);
        }
        let additions: Vec<_> = additions.into_iter().collect();
        let removals: Vec<_> = removals.into_iter().collect();
        let next_revision = old_revision + 1;
        if old_revision == 0 {
            sqlx::query(
                "INSERT INTO commerce_layer_selection_drafts
                 (layer_id,owner_user_id,base_version_id,revision,
                  added_book_ids,removed_book_ids)
                 VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(layer.get())
            .bind(actor.get())
            .bind(current)
            .bind(next_revision)
            .bind(&additions)
            .bind(&removals)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        } else {
            sqlx::query(
                "UPDATE commerce_layer_selection_drafts
                 SET revision=$3,added_book_ids=$4,removed_book_ids=$5,
                     updated_at=now()
                 WHERE layer_id=$1 AND owner_user_id=$2",
            )
            .bind(layer.get())
            .bind(actor.get())
            .bind(next_revision)
            .bind(&additions)
            .bind(&removals)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok((
            LayerSelectionDraft {
                base_version_id: current,
                revision: next_revision,
                additions,
                removals,
            },
            page,
        ))
    }

    pub async fn clear_selection_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        base_version: PositiveId,
        expected_revision: i64,
    ) -> Result<(), LayerError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let _locked: i64 = sqlx::query_scalar(
            "SELECT id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden') FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        // A stale draft may be discarded explicitly. Compare the stored
        // draft's own original version/revision, not the new live version.
        let deleted = sqlx::query(
            "DELETE FROM commerce_layer_selection_drafts
             WHERE layer_id=$1 AND owner_user_id=$2
               AND base_version_id=$3 AND revision=$4",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(base_version.get())
        .bind(expected_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if expected_revision > 0 && deleted.rows_affected() != 1 {
            return Err(LayerError::StaleOffer);
        }
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(())
    }
}
