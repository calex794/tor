//! F-owned persisted share policy for organic reader-end Books.
//! A policy read never collects viewer activity or grants access to a Book.
use super::*;
use crate::services::reader_end_shelf::ShelfSharePolicy;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShelfShareSnapshot {
    pub revision: i64,
    pub policy: ShelfSharePolicy,
}

impl PgRecommendations {
    /// One authoritative version per request; explicit 0% is never replaced by
    /// stale defaults. The administrator CAS owns changes, not this read path.
    pub async fn shelf_share_policy(&self) -> Result<ShelfShareSnapshot, FeedbackError> {
        let values: (i64, i16, i16, i16, i16, i16) = tokio::time::timeout(
            Duration::from_millis(500),
            sqlx::query_as(
                "SELECT revision, author_pct, teammate_pct, other_authors_pct,
                        no_team_author_pct, no_team_other_authors_pct
                 FROM rec_shelf_share_policy WHERE singleton",
            )
            .fetch_one(&self.pool),
        )
        .await
        .map_err(|_| FeedbackError::Unavailable)??;
        let policy = ShelfSharePolicy::validated(
            [
                i32::from(values.1),
                i32::from(values.2),
                i32::from(values.3),
            ],
            [i32::from(values.4), i32::from(values.5)],
        )
        .map_err(|_| FeedbackError::Unavailable)?;
        Ok(ShelfShareSnapshot {
            revision: values.0,
            policy,
        })
    }
}

/// Bounded administrative snapshots; no reader or impression identifiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfShareAuditRow {
    pub revision: i64,
    pub with_team: [u8; 3],
    pub without_team: [u8; 2],
    pub actor_user_id: Option<i64>,
    pub action: String,
    pub changed_at: String,
}

impl PgRecommendations {
    /// The audit is append-only. Keep the result finite and do not expose it to readers.
    pub async fn shelf_share_audit(&self) -> Result<Vec<ShelfShareAuditRow>, FeedbackError> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT revision,author_pct,teammate_pct,other_authors_pct,
                    no_team_author_pct,no_team_other_authors_pct,actor_user_id,action,
                    to_char(changed_at AT TIME ZONE 'UTC',
                            'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS changed_at
             FROM rec_shelf_share_policy_audit ORDER BY revision DESC LIMIT 25",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let ordinary = [
                    i32::from(row.try_get::<i16, _>("author_pct")?),
                    i32::from(row.try_get::<i16, _>("teammate_pct")?),
                    i32::from(row.try_get::<i16, _>("other_authors_pct")?),
                ];
                let alone = [
                    i32::from(row.try_get::<i16, _>("no_team_author_pct")?),
                    i32::from(row.try_get::<i16, _>("no_team_other_authors_pct")?),
                ];
                let policy = ShelfSharePolicy::validated(ordinary, alone)
                    .map_err(|_| FeedbackError::Unavailable)?;
                Ok(ShelfShareAuditRow {
                    revision: row.try_get("revision")?,
                    with_team: policy.with_team(),
                    without_team: policy.without_team(),
                    actor_user_id: row.try_get("actor_user_id")?,
                    action: row.try_get("action")?,
                    changed_at: row.try_get("changed_at")?,
                })
            })
            .collect()
    }

    /// Immutable audit revision used for rollback; it is never edited or deleted.
    pub async fn shelf_share_audit_policy(
        &self,
        revision: i64,
    ) -> Result<Option<ShelfSharePolicy>, FeedbackError> {
        if revision < 1 {
            return Err(FeedbackError::Invalid);
        }
        let values: Option<(i16, i16, i16, i16, i16)> = sqlx::query_as(
            "SELECT author_pct,teammate_pct,other_authors_pct,
                    no_team_author_pct,no_team_other_authors_pct
             FROM rec_shelf_share_policy_audit WHERE revision=$1",
        )
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?;
        values
            .map(|v| {
                ShelfSharePolicy::validated(
                    [i32::from(v.0), i32::from(v.1), i32::from(v.2)],
                    [i32::from(v.3), i32::from(v.4)],
                )
                .map_err(|_| FeedbackError::Unavailable)
            })
            .transpose()
    }

    /// Only the database may authenticate the live administrator and append an
    /// immutable audit row. The browser provides a session cookie, never actor ID.
    /// CAS rejects concurrent submits and repeat clicks with HTTP 409.
    pub async fn set_shelf_share_policy(
        &self,
        session_digest: &[u8; 32],
        expected_revision: i64,
        policy: ShelfSharePolicy,
    ) -> Result<i64, FeedbackError> {
        if expected_revision < 1 {
            return Err(FeedbackError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('tor.rec_share_session',encode($1::bytea,'hex'),true)")
            .bind(session_digest.as_slice())
            .execute(&mut *tx)
            .await?;
        let [author, teammate, other] = policy.with_team();
        let [alone_author, alone_other] = policy.without_team();
        let result = sqlx::query_scalar::<_, i64>(
            "SELECT rec_shelf_share_update($1,$2,$3,$4,$5,$6)",
        )
        .bind(expected_revision)
        .bind(i32::from(author))
        .bind(i32::from(teammate))
        .bind(i32::from(other))
        .bind(i32::from(alone_author))
        .bind(i32::from(alone_other))
        .fetch_one(&mut *tx)
        .await;
        let revision = match result {
            Ok(revision) => revision,
            Err(error) => {
                return Err(match error
                    .as_database_error()
                    .and_then(|db| db.code())
                    .as_deref()
                {
                    Some("P4090") => FeedbackError::Conflict,
                    Some("22023") => FeedbackError::Invalid,
                    Some("42501") => FeedbackError::Forbidden,
                    _ => FeedbackError::Unavailable,
                });
            }
        };
        tx.commit().await?;
        Ok(revision)
    }
}
