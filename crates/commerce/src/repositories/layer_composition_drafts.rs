//! Owner-private staged composition. Draft rows never grant or revoke paid rights.
use super::{
    layer_composition::{validate, LayerChangePreview},
    layers::PgLayers,
};
use crate::domain::layers::LayerError;
use sqlx::Row;
use std::collections::BTreeSet;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerCompositionDraft {
    pub version_id: i64,
    pub revision: i64,
    pub additions: Vec<i64>,
    pub removals: Vec<i64>,
}

fn database_error(_: sqlx::Error) -> LayerError {
    LayerError::Unavailable
}

impl PgLayers {
    /// GET never creates a draft. A version mismatch is surfaced, not discarded.
    pub async fn composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<LayerCompositionDraft, LayerError> {
        let version: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden')",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .ok_or(LayerError::Inaccessible)?;
        let row = sqlx::query(
            "SELECT expected_version_id,revision,additions,removals
             FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(LayerCompositionDraft {
                version_id: version,
                revision: 0,
                additions: Vec::new(),
                removals: Vec::new(),
            });
        };
        let draft_version: i64 = row.try_get("expected_version_id").map_err(database_error)?;
        if draft_version != version {
            return Err(LayerError::StaleOffer);
        }
        Ok(LayerCompositionDraft {
            version_id: draft_version,
            revision: row.try_get("revision").map_err(database_error)?,
            additions: row.try_get("additions").map_err(database_error)?,
            removals: row.try_get("removals").map_err(database_error)?,
        })
    }

    /// Save a whole *visible page* on every navigation. Unseen IDs are untouched.
    /// The client supplies its checked set, but never authoritative membership.
    pub async fn stage_composition_page(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_version: PositiveId,
        expected_revision: i64,
        visible: &[i64],
        selected: &[i64],
    ) -> Result<LayerCompositionDraft, LayerError> {
        let shown: BTreeSet<i64> = visible.iter().copied().collect();
        let checked: BTreeSet<i64> = selected.iter().copied().collect();
        if expected_revision < 0
            || shown.len() != visible.len()
            || checked.len() != selected.len()
            || shown.len() > 50
            || shown.iter().any(|id| *id <= 0)
            || !checked.is_subset(&shown)
        {
            return Err(LayerError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(LayerError::Inaccessible)?;
        if current != expected_version.get() {
            return Err(LayerError::StaleOffer);
        }
        let previous = sqlx::query(
            "SELECT expected_version_id,revision,additions,removals
             FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let (revision, mut adds, mut removes) = if let Some(row) = previous {
            let version: i64 = row.try_get("expected_version_id").map_err(database_error)?;
            let revision: i64 = row.try_get("revision").map_err(database_error)?;
            if version != current || revision != expected_revision {
                return Err(LayerError::StaleOffer);
            }
            (
                revision,
                row.try_get::<Vec<i64>, _>("additions")
                    .map_err(database_error)?
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                row.try_get::<Vec<i64>, _>("removals")
                    .map_err(database_error)?
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
            )
        } else {
            if expected_revision != 0 {
                return Err(LayerError::StaleOffer);
            }
            (0, BTreeSet::new(), BTreeSet::new())
        };
        // Both current members and eligible new books are allowed in the grid.
        // Recheck each posted ID instead of trusting hidden form fields.
        let valid_rows = sqlx::query(
            "SELECT b.id,
                EXISTS(SELECT 1 FROM commerce_layer_version_books vb
                    WHERE vb.layer_version_id=$3 AND vb.book_id=b.id) AS included
             FROM books b
             WHERE b.id=ANY($1::bigint[]) AND b.owner_user_id=$2
               AND (
                 EXISTS(SELECT 1 FROM commerce_layer_version_books vb
                     WHERE vb.layer_version_id=$3 AND vb.book_id=b.id)
                 OR (
                   NOT b.admin_hold AND NOT b.is_deleted
                   AND EXISTS(SELECT 1 FROM book_members m
                       WHERE m.book_id=b.id AND m.user_id=$2
                         AND m.is_active AND m.role='owner')
                   AND NOT EXISTS(SELECT 1 FROM book_members m
                       WHERE m.book_id=b.id AND m.is_active
                         AND (m.user_id<>$2 OR m.role<>'owner'))
                 )
               )",
        )
        .bind(visible)
        .bind(actor.get())
        .bind(current)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        if valid_rows.len() != visible.len() {
            return Err(LayerError::Inaccessible);
        }
        for row in valid_rows {
            let id: i64 = row.try_get("id").map_err(database_error)?;
            let included: bool = row.try_get("included").map_err(database_error)?;
            if checked.contains(&id) {
                removes.remove(&id);
                if !included {
                    adds.insert(id);
                }
            } else {
                adds.remove(&id);
                if included {
                    removes.insert(id);
                }
            }
        }
        let addition_ids: Vec<i64> = adds.into_iter().collect();
        let removal_ids: Vec<i64> = removes.into_iter().collect();
        if addition_ids.len() + removal_ids.len() > 200 {
            return Err(LayerError::Invalid);
        }
        let members: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce_layer_version_books WHERE layer_version_id=$1",
        )
        .bind(current)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if members - removal_ids.len() as i64 + addition_ids.len() as i64 > 100 {
            return Err(LayerError::Invalid);
        }
        let next_revision = revision.checked_add(1).ok_or(LayerError::Invalid)?;
        sqlx::query(
            "INSERT INTO commerce_layer_composition_drafts
                 (layer_id,owner_user_id,expected_version_id,revision,additions,removals)
             VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT(layer_id) DO UPDATE SET
                 expected_version_id=EXCLUDED.expected_version_id,
                 revision=EXCLUDED.revision,
                 additions=EXCLUDED.additions,
                 removals=EXCLUDED.removals,
                 updated_at=now()",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(current)
        .bind(next_revision)
        .bind(&addition_ids)
        .bind(&removal_ids)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(LayerCompositionDraft {
            version_id: current,
            revision: next_revision,
            additions: addition_ids,
            removals: removal_ids,
        })
    }

    /// Discard is owner scoped and CAS guarded, including an obsolete draft.
    pub async fn discard_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_revision: i64,
    ) -> Result<(), LayerError> {
        if expected_revision < 0 {
            return Err(LayerError::Invalid);
        }
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2)",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if !owned {
            return Err(LayerError::Inaccessible);
        }
        if expected_revision == 0 {
            return Ok(());
        }
        let deleted = sqlx::query(
            "DELETE FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 AND revision=$3",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(expected_revision)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if deleted.rows_affected() != 1 {
            return Err(LayerError::StaleOffer);
        }
        Ok(())
    }

    /// Reset only a draft based on an obsolete composition version.
    /// A late stale-page request cannot destroy a newer valid draft.
    pub async fn discard_stale_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<(), LayerError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden') FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(LayerError::Inaccessible)?;
        sqlx::query(
            "DELETE FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 AND expected_version_id<>$3",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(current)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    /// Apply exactly the reviewed draft in the same transaction that deletes it.
    /// Lock order: layer, draft, book rows. Both version and draft revision are CAS.
    pub async fn apply_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        draft_revision: i64,
        expected_buyers: (i64, i64),
    ) -> Result<i64, LayerError> {
        if draft_revision <= 0 || expected_buyers.0 < 0 || expected_buyers.1 < 0 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(LayerError::Inaccessible)?;
        if current != version.get() {
            return Err(LayerError::StaleOffer);
        }
        let row = sqlx::query(
            "SELECT expected_version_id,revision,additions,removals
             FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(LayerError::StaleOffer)?;
        let saved_version: i64 = row.try_get("expected_version_id").map_err(database_error)?;
        let saved_revision: i64 = row.try_get("revision").map_err(database_error)?;
        if saved_version != current || saved_revision != draft_revision {
            return Err(LayerError::StaleOffer);
        }
        let additions: Vec<i64> = row.try_get("additions").map_err(database_error)?;
        let removals: Vec<i64> = row.try_get("removals").map_err(database_error)?;
        validate(&additions, &removals)?;
        let mut ids: Vec<i64> = additions.iter().chain(&removals).copied().collect();
        ids.sort_unstable();
        let locked: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=ANY($1::bigint[]) ORDER BY id FOR UPDATE",
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        if locked != ids {
            return Err(LayerError::Inaccessible);
        }
        let snapshot: LayerChangePreview = self
            .inspect_change(&mut tx, actor, layer, version, &additions, &removals)
            .await?;
        if (snapshot.active_buyers, snapshot.lifetime_buyers) != expected_buyers {
            return Err(LayerError::StaleOffer);
        }
        let next: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_versions(layer_id,version_no)
             SELECT $1,version_no+1 FROM commerce_layer_versions WHERE id=$2
             RETURNING id",
        )
        .bind(layer.get())
        .bind(current)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,book_id FROM commerce_layer_version_books
             WHERE layer_version_id=$2 AND NOT (book_id=ANY($3::bigint[]))",
        )
        .bind(next)
        .bind(current)
        .bind(&removals)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,src.book_id FROM unnest($2::bigint[]) AS src(book_id)",
        )
        .bind(next)
        .bind(&additions)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
            .bind(layer.get())
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query(
            "DELETE FROM commerce_layer_composition_drafts
             WHERE layer_id=$1 AND owner_user_id=$2 AND revision=$3",
        )
        .bind(layer.get())
        .bind(actor.get())
        .bind(saved_revision)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(next)
    }
}
