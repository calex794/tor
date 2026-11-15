//! Explicit settings expire independently of optional recommendation collection.
use super::*;

impl PgRecommendations {
    /// Safe for the always-on privacy worker, including REC/AUTH flag-off mode.
    /// Concurrent workers skip locked accounts; no GET or analytics scope is needed.
    pub async fn prune_preferences(&self) -> Result<u64, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        // Both row marks are nonblocking: a busy user or setting head does not
        // consume the batch limit. No physical join lock order is assumed, and
        // neither mark can wait behind an account deletion holding the other.
        let actors: Vec<i64> = sqlx::query_scalar(
            "SELECT p.user_id FROM users u
             JOIN rec_preference_accounts p ON p.user_id=u.id
             WHERE EXISTS(
                 SELECT 1 FROM rec_book_preferences f WHERE f.user_id=p.user_id
                   AND f.expires_at<now()-interval '1 hour'
             ) OR EXISTS(
                 SELECT 1 FROM rec_user_preference_commands c WHERE c.user_id=p.user_id
                   AND c.created_at<now()-interval '30 days'
             )
             ORDER BY p.user_id LIMIT 32
             FOR SHARE OF u SKIP LOCKED FOR UPDATE OF p SKIP LOCKED",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut erased = 0;
        for actor in actors {
            let books = sqlx::query(
                "DELETE FROM rec_book_preferences
                 WHERE user_id=$1 AND book_id IN(
                     SELECT book_id FROM rec_book_preferences
                     WHERE user_id=$1 AND expires_at<now()-interval '1 hour'
                     ORDER BY expires_at,book_id LIMIT 1000
                 )",
            )
            .bind(actor)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if books > 0 {
                // A new stale command cannot recreate an erased choice.
                sqlx::query(
                    "UPDATE rec_preference_accounts SET feedback_epoch=feedback_epoch+1
                     WHERE user_id=$1",
                )
                .bind(actor)
                .execute(&mut *tx)
                .await?;
            }
            erased += books;
            erased += sqlx::query(
                "DELETE FROM rec_user_preference_commands
                 WHERE user_id=$1 AND command_key IN(
                     SELECT command_key FROM rec_user_preference_commands
                     WHERE user_id=$1 AND created_at<now()-interval '30 days'
                     ORDER BY created_at,command_key LIMIT 1000
                 )",
            )
            .bind(actor)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(erased)
    }
}
