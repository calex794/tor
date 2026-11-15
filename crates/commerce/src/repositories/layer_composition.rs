//! Owner-scoped, atomic multi-book changes. A preview never mutates paid rights.
//! The apply path locks the package before checking the version and affected readers.
use super::layers::PgLayers;
use crate::domain::layers::LayerError;
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone, Debug)]
pub struct LayerBookTile {
    pub id: i64,
    pub title: String,
    pub has_cover: bool,
    pub included: bool,
}

#[derive(Clone, Debug)]
pub struct LayerBookTilePage {
    pub books: Vec<LayerBookTile>,
    pub after: Option<i64>,
    pub next_after: Option<i64>,
    /// Stable current-package cover regardless of filtering or page cursor.
    pub hero_cover_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerChangeBook {
    pub id: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerChangePreview {
    pub version_id: i64,
    pub additions: Vec<LayerChangeBook>,
    pub removals: Vec<LayerChangeBook>,
    /// Unique buyers with an active, non-revoked paid term in this package.
    /// They may have other independent entitlements or age restrictions.
    pub active_buyers: i64,
    pub lifetime_buyers: i64,
}

fn validate(additions: &[i64], removals: &[i64]) -> Result<(), LayerError> {
    if additions.len() + removals.len() > 100
        || additions.is_empty() && removals.is_empty()
        || additions.iter().chain(removals).any(|id| *id <= 0)
    {
        return Err(LayerError::Invalid);
    }
    let all: BTreeSet<_> = additions.iter().chain(removals).copied().collect();
    if all.len() != additions.len() + removals.len() {
        return Err(LayerError::Invalid);
    }
    Ok(())
}

impl PgLayers {
    /// A bounded, searchable grid. Current members are shown even if they
    /// ceased to qualify for re-adding; eligible new books share the same grid.
    pub async fn composition_tiles(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        search: &str,
        limit: u16,
    ) -> Result<LayerBookTilePage, LayerError> {
        let search = search.trim();
        if !(1..=50).contains(&limit) || search.chars().count() > 100 {
            return Err(LayerError::Invalid);
        }
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden')",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let hero_cover_id: Option<i64> = sqlx::query_scalar(
            "SELECT b.id
             FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1 AND b.visibility='public'
               AND b.visibility_status='active' AND NOT b.is_deleted
               AND EXISTS(
                 SELECT 1 FROM book_cover_slots s
                 JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
                 JOIN cover_variants v ON v.job_id=j.id AND v.variant_name='card'
                 WHERE s.book_id=b.id AND j.status='ready'
               )
             ORDER BY b.id LIMIT 1",
        )
        .bind(current)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let rows = sqlx::query(
            "SELECT b.id,b.title,
               EXISTS(SELECT 1 FROM commerce_layer_version_books vb
                      WHERE vb.layer_version_id=$2 AND vb.book_id=b.id) AS included,
               (b.visibility='public' AND b.visibility_status='active'
                AND NOT b.is_deleted AND EXISTS(
                  SELECT 1 FROM book_cover_slots s
                  JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
                  JOIN cover_variants v ON v.job_id=j.id AND v.variant_name='card'
                  WHERE s.book_id=b.id AND j.status='ready'
                )) AS has_cover
             FROM books b
             WHERE b.owner_user_id=$1 AND b.id>$3
               AND ($4='' OR POSITION(lower($4) IN lower(b.title))>0)
               AND (
                 EXISTS(SELECT 1 FROM commerce_layer_version_books vb
                        WHERE vb.layer_version_id=$2 AND vb.book_id=b.id)
                 OR (
                   NOT b.admin_hold AND NOT b.is_deleted
                   AND EXISTS(SELECT 1 FROM book_members m
                              WHERE m.book_id=b.id AND m.user_id=$1
                                AND m.is_active AND m.role='owner')
                   AND NOT EXISTS(SELECT 1 FROM book_members m
                                  WHERE m.book_id=b.id AND m.is_active
                                    AND (m.user_id<>$1 OR m.role<>'owner'))
                 )
               )
             ORDER BY b.id LIMIT $5",
        )
        .bind(actor.get())
        .bind(current)
        .bind(after.map_or(0, PositiveId::get))
        .bind(search)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let more = rows.len() > usize::from(limit);
        let mut books = Vec::new();
        for row in rows.into_iter().take(usize::from(limit)) {
            books.push(LayerBookTile {
                id: row.try_get("id").map_err(|_| LayerError::Unavailable)?,
                title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
                has_cover: row
                    .try_get("has_cover")
                    .map_err(|_| LayerError::Unavailable)?,
                included: row
                    .try_get("included")
                    .map_err(|_| LayerError::Unavailable)?,
            });
        }
        Ok(LayerBookTilePage {
            hero_cover_id,
            after: after.map(PositiveId::get),
            next_after: if more {
                books.last().map(|b| b.id)
            } else {
                None
            },
            books,
        })
    }

    /// The transaction sees a current immutable version; the apply method
    /// additionally holds the package lock for conflict-free publication.
    async fn inspect_change(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        actor: PositiveId,
        layer: PositiveId,
        expected_version: PositiveId,
        additions: &[i64],
        removals: &[i64],
    ) -> Result<LayerChangePreview, LayerError> {
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active'",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        if current != expected_version.get() {
            return Err(LayerError::StaleOffer);
        }
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce_layer_version_books WHERE layer_version_id=$1",
        )
        .bind(current)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if existing - removals.len() as i64 + additions.len() as i64 > 100 {
            return Err(LayerError::Invalid);
        }
        let added = sqlx::query(
            "SELECT b.id,b.title FROM books b
             WHERE b.id=ANY($1::bigint[]) AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS(SELECT 1 FROM book_members m
                          WHERE m.book_id=b.id AND m.user_id=$2
                            AND m.is_active AND m.role='owner')
               AND NOT EXISTS(SELECT 1 FROM book_members m
                              WHERE m.book_id=b.id AND m.is_active
                                AND (m.user_id<>$2 OR m.role<>'owner'))
               AND NOT EXISTS(SELECT 1 FROM commerce_layer_version_books vb
                              WHERE vb.layer_version_id=$3 AND vb.book_id=b.id)
             ORDER BY b.id",
        )
        .bind(additions)
        .bind(actor.get())
        .bind(current)
        .fetch_all(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let removed = sqlx::query(
            "SELECT b.id,b.title FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1 AND vb.book_id=ANY($2::bigint[])
             ORDER BY b.id",
        )
        .bind(current)
        .bind(removals)
        .fetch_all(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if added.len() != additions.len() || removed.len() != removals.len() {
            return Err(LayerError::Inaccessible);
        }
        let audience: (i64, i64) = sqlx::query_as(
            "SELECT count(DISTINCT t.user_id),
                    count(DISTINCT t.user_id) FILTER (WHERE t.term_kind='lifetime')
             FROM commerce_layer_terms t
             WHERE t.layer_id=$1 AND t.started_at<=now()
               AND (t.expires_at IS NULL OR t.expires_at>now())
               AND NOT EXISTS(SELECT 1 FROM commerce_layer_term_revocations r
                              WHERE r.term_id=t.id)",
        )
        .bind(layer.get())
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let books = |rows: Vec<sqlx::postgres::PgRow>| {
            rows.into_iter()
                .map(|row| {
                    Ok(LayerChangeBook {
                        id: row.try_get("id").map_err(|_| LayerError::Unavailable)?,
                        title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
                    })
                })
                .collect::<Result<Vec<_>, LayerError>>()
        };
        Ok(LayerChangePreview {
            version_id: current,
            additions: books(added)?,
            removals: books(removed)?,
            active_buyers: audience.0,
            lifetime_buyers: audience.1,
        })
    }

    pub async fn preview_composition(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
    ) -> Result<LayerChangePreview, LayerError> {
        validate(additions, removals)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        self.inspect_change(&mut tx, actor, layer, version, additions, removals)
            .await
    }

    /// One and only one immutable revision for a validated batch. The
    /// audience counts must match the preview, otherwise ask for re-preview.
    pub async fn apply_composition(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
        expected_buyers: (i64, i64),
    ) -> Result<i64, LayerError> {
        self.apply_composition_internal(
            actor, layer, version, additions, removals, (expected_buyers, None),
        ).await
    }

    /// A reviewed draft must still be the same draft at the commit boundary.
    pub async fn apply_staged_composition(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
        approved: ((i64, i64), i64),
    ) -> Result<i64, LayerError> {
        self.apply_composition_internal(
            actor, layer, version, additions, removals, (approved.0, Some(approved.1)),
        ).await
    }

    async fn apply_composition_internal(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
        approved: ((i64, i64), Option<i64>),
    ) -> Result<i64, LayerError> {
        let ((expected_active_buyers, expected_lifetime_buyers), draft_revision) = approved;
        validate(additions, removals)?;
        if expected_active_buyers < 0 || expected_lifetime_buyers < 0 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let locked: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if locked.is_none() {
            return Err(LayerError::Inaccessible);
        }
        if let Some(expected_revision) = draft_revision {
            let draft = sqlx::query(
                "SELECT base_version_id,revision,added_book_ids,removed_book_ids
                 FROM commerce_layer_selection_drafts
                 WHERE layer_id=$1 AND owner_user_id=$2 FOR UPDATE",
            )
            .bind(layer.get()).bind(actor.get())
            .fetch_optional(&mut *tx).await
            .map_err(|_| LayerError::Unavailable)?
            .ok_or(LayerError::StaleOffer)?;
            let stored_revision: i64 = draft.try_get("revision")
                .map_err(|_| LayerError::Unavailable)?;
            let stored_version: i64 = draft.try_get("base_version_id")
                .map_err(|_| LayerError::Unavailable)?;
            let stored_add: Vec<i64> = draft.try_get("added_book_ids")
                .map_err(|_| LayerError::Unavailable)?;
            let stored_remove: Vec<i64> = draft.try_get("removed_book_ids")
                .map_err(|_| LayerError::Unavailable)?;
            let requested_add = additions.iter().copied().collect::<BTreeSet<_>>();
            let requested_remove = removals.iter().copied().collect::<BTreeSet<_>>();
            if stored_revision != expected_revision || stored_version != version.get()
                || stored_add.into_iter().collect::<BTreeSet<_>>() != requested_add
                || stored_remove.into_iter().collect::<BTreeSet<_>>() != requested_remove
            {
                return Err(LayerError::StaleOffer);
            }
        }
        let mut ids: Vec<i64> = additions.iter().chain(removals).copied().collect();
        ids.sort_unstable();
        // The canonical Book row also serializes ownership and team changes.
        let locked_books: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=ANY($1::bigint[]) ORDER BY id FOR UPDATE",
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if locked_books != ids {
            return Err(LayerError::Inaccessible);
        }
        let snapshot = self
            .inspect_change(&mut tx, actor, layer, version, additions, removals)
            .await?;
        if snapshot.active_buyers != expected_active_buyers
            || snapshot.lifetime_buyers != expected_lifetime_buyers
        {
            return Err(LayerError::StaleOffer);
        }
        let next: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_versions(layer_id,version_no)
             SELECT $1,version_no+1 FROM commerce_layer_versions WHERE id=$2
             RETURNING id",
        )
        .bind(layer.get())
        .bind(version.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,book_id FROM commerce_layer_version_books
             WHERE layer_version_id=$2 AND NOT (book_id=ANY($3::bigint[]))",
        )
        .bind(next)
        .bind(version.get())
        .bind(removals)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,src.book_id FROM unnest($2::bigint[]) AS src(book_id)",
        )
        .bind(next)
        .bind(additions)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
            .bind(layer.get())
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        if draft_revision.is_some() {
            sqlx::query(
                "DELETE FROM commerce_layer_selection_drafts
                 WHERE layer_id=$1 AND owner_user_id=$2",
            )
            .bind(layer.get()).bind(actor.get())
            .execute(&mut *tx).await.map_err(|_| LayerError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(next)
    }
}
