//! AB-08/09: bounded per-actor action admission, shared across web processes.
//! Independent of login/spray caps. Replays do not consume additional quota.
use super::PgAccounts;

/// The donor's authenticated `comments` scope: 30 actions per 60 seconds.
const COMMENT_LIMIT: i32 = 30;
const WINDOW_SECONDS: i32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedActionAdmission {
    Allowed,
    Limited { retry_after: u32 },
}

impl PgAccounts {
    /// `command_digest` is SHA-256 of the *validated random* idempotency key.
    /// Repeating that key is allowed through to the domain's existing payload
    /// comparison; it never bypasses authorization or changes the old comment.
    ///
    /// Every writer locks one (scope,actor) row in PostgreSQL. Separate Axum
    /// instances cannot each spend 30 actions on the same account.
    pub async fn permit_comment_action(
        &self,
        actor_id: i64,
        command_digest: &[u8; 32],
    ) -> Result<ScopedActionAdmission, sqlx::Error> {
        if actor_id <= 0 {
            return Err(sqlx::Error::Protocol("invalid comment actor".to_owned()));
        }
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO scoped_action_windows(scope,actor_id)
             VALUES('comments',$1)
             ON CONFLICT (scope,actor_id) DO NOTHING",
        )
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;

        let (mut admitted, remaining): (i32, i32) = sqlx::query_as(
            "SELECT admitted,
                GREATEST(0,CEIL(EXTRACT(EPOCH FROM (
                    period_started + ($2::integer * INTERVAL '1 second')
                    - clock_timestamp()
                ))))::integer
             FROM scoped_action_windows
             WHERE scope='comments' AND actor_id=$1 FOR UPDATE",
        )
        .bind(actor_id)
        .bind(WINDOW_SECONDS)
        .fetch_one(&mut *tx)
        .await?;

        if remaining == 0 {
            // Keep only keys from the *current* window. The window lock
            // serializes reset with writers on another PostgreSQL connection.
            sqlx::query(
                "DELETE FROM scoped_action_command_keys
                 WHERE scope='comments' AND actor_id=$1",
            )
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE scoped_action_windows
                 SET period_started=clock_timestamp(), admitted=0,
                     touched_at=clock_timestamp()
                 WHERE scope='comments' AND actor_id=$1",
            )
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;
            admitted = 0;
        }

        let repeated: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM scoped_action_command_keys
                 WHERE scope='comments' AND actor_id=$1 AND command_digest=$2
             )",
        )
        .bind(actor_id)
        .bind(command_digest.as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if repeated {
            tx.commit().await?;
            return Ok(ScopedActionAdmission::Allowed);
        }
        if admitted >= COMMENT_LIMIT {
            // No new admission, no domain mutation, no new key. Drop
            // transaction to release the row lock without changing state.
            return Ok(ScopedActionAdmission::Limited {
                retry_after: remaining.max(1) as u32,
            });
        }

        sqlx::query(
            "INSERT INTO scoped_action_command_keys(scope,actor_id,command_digest)
             VALUES('comments',$1,$2)",
        )
        .bind(actor_id)
        .bind(command_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE scoped_action_windows
             SET admitted=admitted+1, touched_at=clock_timestamp()
             WHERE scope='comments' AND actor_id=$1",
        )
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ScopedActionAdmission::Allowed)
    }
}
