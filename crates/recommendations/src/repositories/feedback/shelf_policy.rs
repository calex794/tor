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

    /// Commit an administrator-approved policy in one CAS transaction.
    /// The SQL SECURITY DEFINER function revalidates the live A session and
    /// role; this API never trusts a caller-supplied actor id.
    pub async fn set_shelf_share_policy(
        &self,
        session_digest: &[u8; 32],
        expected_revision: i64,
        with_team: [i32; 3],
        without_team: [i32; 2],
    ) -> Result<ShelfShareSnapshot, FeedbackError> {
        let policy = ShelfSharePolicy::validated(with_team, without_team)
            .map_err(|_| FeedbackError::Invalid)?;
        if expected_revision < 1 {
            return Err(FeedbackError::Invalid);
        }
        let transaction = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET LOCAL lock_timeout='75ms'")
                .execute(&mut *tx)
                .await?;
            sqlx::query("SET LOCAL statement_timeout='750ms'")
                .execute(&mut *tx)
                .await?;
            sqlx::query("SELECT set_config('tor.rec_share_session',encode($1::bytea,'hex'),true)")
                .bind(session_digest.as_slice())
                .execute(&mut *tx)
                .await?;
            let revision =
                sqlx::query_scalar::<_, i64>("SELECT rec_shelf_share_update($1,$2,$3,$4,$5,$6)")
                    .bind(expected_revision)
                    .bind(with_team[0])
                    .bind(with_team[1])
                    .bind(with_team[2])
                    .bind(without_team[0])
                    .bind(without_team[1])
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|error| {
                        match error
                            .as_database_error()
                            .and_then(|db| db.code())
                            .as_deref()
                        {
                            Some("42501") => FeedbackError::Forbidden,
                            Some("22023") => FeedbackError::Invalid,
                            Some("P4090") => FeedbackError::Conflict,
                            _ => FeedbackError::Unavailable,
                        }
                    })?;
            tx.commit().await.map_err(FeedbackError::from)?;
            Ok::<i64, FeedbackError>(revision)
        };
        let revision = tokio::time::timeout(Duration::from_millis(950), transaction)
            .await
            .map_err(|_| FeedbackError::Unavailable)??;
        Ok(ShelfShareSnapshot { revision, policy })
    }
}
