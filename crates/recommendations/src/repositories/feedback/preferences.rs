use super::*;
use crate::{
    domain::feedback::{token_digest, Choice, FeedbackReceipt, HiddenBook},
    services::feedback::{validate_actor, validate_revision},
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgRecommendations {
    pub async fn set_feedback(
        &self,
        actor: i64,
        book: i64,
        choice: Choice,
        expected: i64,
        command: &str,
    ) -> Result<FeedbackReceipt, FeedbackError> {
        validate_actor(actor)?;
        validate_actor(book)?;
        validate_revision(expected)?;
        let key = token_digest(command)?;
        let hidden = choice.hidden();
        let payload: [u8; 32] =
            Sha256::digest(format!("feedback:{book}:{hidden}:{expected}")).into();
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, actor).await?;
        let epoch: i64 =
            sqlx::query_scalar("SELECT feedback_epoch FROM rec_subjects WHERE user_id=$1")
                .bind(actor)
                .fetch_one(&mut *tx)
                .await?;
        // The original subject is erased on opt-out; its old CAS must never
        // become valid again for a fresh subject or after preference TTL.
        if expected / REVISION_STRIDE != epoch {
            return Err(FeedbackError::Conflict);
        }
        let book_revision = expected % REVISION_STRIDE;
        if let Some(row) = sqlx::query("SELECT payload_hash,result_revision,command_type FROM rec_preference_commands WHERE user_id=$1 AND command_key=$2")
            .bind(actor).bind(key.as_slice()).fetch_optional(&mut *tx).await? {
            if row.try_get::<Vec<u8>,_>("payload_hash")? != payload || row.try_get::<String,_>("command_type")? != "feedback" {
                return Err(FeedbackError::Conflict);
            }
            return Ok(FeedbackReceipt { revision: row.try_get("result_revision")?, hidden });
        }
        let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM rec_preference_commands WHERE user_id=$1 AND created_at>now()-interval '1 minute'")
            .bind(actor).fetch_one(&mut *tx).await?;
        if commands >= 30 {
            return Err(FeedbackError::Limited);
        }
        let visible: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_public_books WHERE id=$1)")
                .bind(book)
                .fetch_one(&mut *tx)
                .await?;
        let old: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM rec_feedback_state WHERE subject_id=$1::uuid AND book_id=$2",
        )
        .bind(&subject)
        .bind(book)
        .fetch_optional(&mut *tx)
        .await?;
        if hidden && !visible || !hidden && old.is_none() && !visible {
            return Err(FeedbackError::NotFound);
        }
        if old.unwrap_or(0) != book_revision || book_revision >= REVISION_STRIDE - 1 {
            return Err(FeedbackError::Conflict);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rec_feedback_state WHERE subject_id=$1::uuid")
                .bind(&subject)
                .fetch_one(&mut *tx)
                .await?;
        if old.is_none() && count >= 1000 {
            return Err(FeedbackError::Limited);
        }
        sqlx::query("INSERT INTO rec_feedback_state(subject_id,book_id,hidden,revision) VALUES($1::uuid,$2,$3,$4) ON CONFLICT(subject_id,book_id) DO UPDATE SET hidden=EXCLUDED.hidden,revision=EXCLUDED.revision,expires_at=now()+interval '30 days'")
            .bind(&subject).bind(book).bind(hidden).bind(book_revision+1).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO rec_preference_commands(user_id,command_key,payload_hash,result_revision,command_type) VALUES($1,$2,$3,$4,'feedback')")
            .bind(actor).bind(key.as_slice()).bind(payload.as_slice()).bind(expected+1).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(FeedbackReceipt {
            revision: expected + 1,
            hidden,
        })
    }

    pub async fn hidden_books(
        &self,
        actor: i64,
        after: i64,
    ) -> Result<Vec<HiddenBook>, FeedbackError> {
        validate_actor(actor)?;
        if after < 0 {
            return Err(FeedbackError::Invalid);
        }
        let rows = sqlx::query("SELECT f.book_id::text,b.title,(s.feedback_epoch*$3+f.revision) AS revision FROM rec_feedback_state f JOIN rec_subjects s ON s.subject_id=f.subject_id JOIN rec_public_books b ON b.id=f.book_id WHERE s.user_id=$1 AND s.enabled AND f.hidden AND f.expires_at>now() AND f.book_id>$2 ORDER BY f.book_id LIMIT 20")
            .bind(actor).bind(after).bind(REVISION_STRIDE).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(HiddenBook {
                    book_id: r.try_get("book_id")?,
                    title: r.try_get("title")?,
                    revision: r.try_get("revision")?,
                })
            })
            .collect()
    }
}
