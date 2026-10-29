//! D-05 x B-10: finance provenance boundary for Layer purchase.
//! Book row locks and immutable share revisions prevent a partial money commit.
use crate::domain::layers::LayerError;
use sqlx::{Postgres, Row, Transaction};

/// Called only after the buyer wallet lock, before the first wallet debit.
/// The caller already holds FOR SHARE on all historical Layer Book rows.
pub(crate) async fn preflight_term_economics(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    layer: i64,
    base_version: i64,
    kind: &str,
) -> Result<i64, LayerError> {
    // B-10: resolve the SAME uninterrupted term origin as the insertion
    // below while the buyer wallet is locked (concurrent renewals serialize).
    // The Layer already holds FOR SHARE on ALL historical Book rows, which
    // fences B's share-revision CAS until the complete money commit.
    let origin_version = if kind == "lifetime" {
        base_version
    } else {
        let previous: Option<i64> = sqlx::query_scalar(
            "SELECT t.access_origin_version_id FROM commerce_layer_terms t
             WHERE t.user_id=$1 AND t.layer_id=$2
               AND t.term_kind='prepaid_30d' AND t.expires_at>now()
               AND NOT EXISTS(
                 SELECT 1 FROM commerce_layer_term_revocations r
                 WHERE r.term_id=t.id
               )
             ORDER BY t.expires_at DESC,t.id DESC LIMIT 1",
        )
        .bind(actor)
        .bind(layer)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        previous.unwrap_or(base_version)
    };
    // All historical Book rows are already SHARE-locked by the checkout
    // transaction. Never debit a Layer buyer while any included or inherited
    // Book is administratively held or deleted.
    let unavailable: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM commerce_layer_versions origin
             JOIN commerce_layer_versions base ON base.layer_id=origin.layer_id
             JOIN commerce_layer_versions v ON v.layer_id=origin.layer_id
               AND v.version_no BETWEEN origin.version_no AND base.version_no
             JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
             JOIN books b ON b.id=vb.book_id
             WHERE origin.id=$1 AND base.id=$2 AND origin.layer_id=$3
               AND (b.admin_hold OR b.is_deleted)
         )",
    )
    .bind(origin_version)
    .bind(base_version)
    .bind(layer)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    if unavailable {
        return Err(LayerError::Inaccessible);
    }
    let required_books: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT vb.book_id)
         FROM commerce_layer_versions origin
         JOIN commerce_layer_versions base
           ON base.layer_id=origin.layer_id
         JOIN commerce_layer_versions v ON v.layer_id=origin.layer_id
           AND v.version_no BETWEEN origin.version_no AND base.version_no
         JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
         WHERE origin.id=$1 AND base.id=$2 AND origin.layer_id=$3",
    )
    .bind(origin_version)
    .bind(base_version)
    .bind(layer)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    let economics = sqlx::query(
        "SELECT DISTINCT vb.book_id, s.current_revision,
           (SELECT COALESCE(SUM(a.share_percent),0)::BIGINT
            FROM revenue_share_assignments a
            WHERE a.book_id=vb.book_id AND a.revision=s.current_revision
           ) AS share_total
         FROM commerce_layer_versions origin
         JOIN commerce_layer_versions base ON base.layer_id=origin.layer_id
         JOIN commerce_layer_versions v ON v.layer_id=origin.layer_id
           AND v.version_no BETWEEN origin.version_no AND base.version_no
         JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
         JOIN revenue_share_states s ON s.book_id=vb.book_id
         WHERE origin.id=$1 AND base.id=$2 AND origin.layer_id=$3
         ORDER BY vb.book_id",
    )
    .bind(origin_version)
    .bind(base_version)
    .bind(layer)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    if required_books == 0 || economics.len() as i64 != required_books {
        return Err(LayerError::Inaccessible);
    }
    for record in &economics {
        if record
            .try_get::<i64, _>("share_total")
            .map_err(|_| LayerError::Unavailable)?
            != 100
        {
            return Err(LayerError::Inaccessible);
        }
    }

    Ok(required_books)
}

/// Record the exact current percentage schedule for every Book in the
/// immutable paid origin..base package, before the transaction commits.
pub(crate) async fn freeze_term_economics(
    tx: &mut Transaction<'_, Postgres>,
    term: i64,
    required_books: i64,
) -> Result<(), LayerError> {
    // Freeze the actual recipient percentages and economics revision for
    // EACH Book in this purchased origin..base version range. New future
    // Layer versions may extend reader rights, but cannot silently rewrite
    // previously paid economic provenance. These are not payout entries.
    let inserted = sqlx::query(
        "INSERT INTO commerce_layer_term_share_snapshots
         (term_id,book_id,share_revision)
         SELECT $1,vb.book_id,s.current_revision
         FROM commerce_layer_terms t
         JOIN commerce_layer_versions origin
           ON origin.id=t.access_origin_version_id AND origin.layer_id=t.layer_id
         JOIN commerce_layer_versions base
           ON base.id=t.base_version_id AND base.layer_id=t.layer_id
         JOIN commerce_layer_versions v ON v.layer_id=t.layer_id
           AND v.version_no BETWEEN origin.version_no AND base.version_no
         JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
         JOIN revenue_share_states s ON s.book_id=vb.book_id
         WHERE t.id=$1
         GROUP BY vb.book_id,s.current_revision",
    )
    .bind(term)
    .execute(&mut **tx)
    .await
    .map_err(|_| LayerError::Unavailable)?
    .rows_affected();
    if inserted != required_books as u64 {
        return Err(LayerError::Unavailable);
    }
    sqlx::query(
        "INSERT INTO commerce_layer_term_share_lines
         (term_id,book_id,user_id,share_percent)
         SELECT s.term_id,s.book_id,a.user_id,a.share_percent
         FROM commerce_layer_term_share_snapshots s
         JOIN revenue_share_assignments a
           ON a.book_id=s.book_id AND a.revision=s.share_revision
         WHERE s.term_id=$1",
    )
    .bind(term)
    .execute(&mut **tx)
    .await
    .map_err(|_| LayerError::Unavailable)?;

    Ok(())
}
