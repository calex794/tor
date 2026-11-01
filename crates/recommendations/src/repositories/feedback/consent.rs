use super::*;
use crate::{
    domain::feedback::{token_digest, ConsentState},
    services::feedback::{validate_actor, validate_revision},
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgRecommendations {
    pub async fn consent(&self, actor: i64) -> Result<ConsentState, FeedbackError> {
        validate_actor(actor)?;
        let row = sqlx::query("SELECT enabled,revision FROM rec_subjects WHERE user_id=$1")
            .bind(actor)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(ConsentState {
                enabled: row.try_get("enabled")?,
                revision: row.try_get("revision")?,
            }),
            None => Ok(ConsentState {
                enabled: false,
                revision: 0,
            }),
        }
    }

    pub async fn set_consent(
        &self,
        actor: i64,
        enabled: bool,
        revision: i64,
        command: &str,
    ) -> Result<ConsentState, FeedbackError> {
        validate_actor(actor)?;
        validate_revision(revision)?;
        let key = token_digest(command)?;
        let payload: [u8; 32] = Sha256::digest(format!("consent:{enabled}:{revision}")).into();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO rec_subjects(user_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        let state = sqlx::query("SELECT subject_id::text,revision,enabled FROM rec_subjects WHERE user_id=$1 FOR UPDATE")
            .bind(actor).fetch_one(&mut *tx).await?;
        if let Some(receipt) = sqlx::query("SELECT payload_hash,result_revision,command_type FROM rec_preference_commands WHERE user_id=$1 AND command_key=$2")
            .bind(actor).bind(key.as_slice()).fetch_optional(&mut *tx).await? {
            let hash: Vec<u8> = receipt.try_get("payload_hash")?;
            let kind: String = receipt.try_get("command_type")?;
            if hash != payload || kind != "consent" {
                return Err(FeedbackError::IdempotencyConflict);
            }
            return Ok(ConsentState { enabled, revision: receipt.try_get("result_revision")? });
        }
        if state.try_get::<i64, _>("revision")? != revision {
            return Err(FeedbackError::Conflict);
        }
        if state.try_get::<bool, _>("enabled")? == enabled {
            sqlx::query(
                "UPDATE rec_subjects SET revision=revision+1,changed_at=now() WHERE user_id=$1",
            )
            .bind(actor)
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO rec_preference_commands(user_id,command_key,payload_hash,result_revision,command_type) VALUES($1,$2,$3,$4,'consent')")
                .bind(actor).bind(key.as_slice()).bind(payload.as_slice()).bind(revision+1).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(ConsentState {
                enabled,
                revision: revision + 1,
            });
        }
        let subject: String = state.try_get("subject_id")?;
        sqlx::query("INSERT INTO rec_dirty_buckets(bucket_date,book_id) SELECT DISTINCT cohort_day,book_id FROM rec_assignments WHERE subject_id=$1::uuid ON CONFLICT DO NOTHING")
            .bind(&subject).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM rec_assignments WHERE subject_id=$1::uuid")
            .bind(&subject)
            .execute(&mut *tx)
            .await?;
        // Rotate the organic consent epoch in the same transaction. Rebuild
        // any still-materialized daily buckets without the withdrawn reader.
        sqlx::query(
            "INSERT INTO organic_discovery_dirty(bucket_date,book_id)
             SELECT DISTINCT cohort_day,book_id FROM organic_discovery_touches
             WHERE subject_id=$1::uuid ON CONFLICT DO NOTHING",
        )
        .bind(&subject)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM organic_discovery_touches WHERE subject_id=$1::uuid")
            .bind(&subject)
            .execute(&mut *tx)
            .await?;
        // Erase the consented reader's first C facts too; both attribution
        // channels and the unattributed denominator must honor opt-out.
        sqlx::query(
            "INSERT INTO organic_discovery_dirty(bucket_date,book_id)
             SELECT DISTINCT bucket_date,book_id FROM organic_discovery_unattributed_reads
             WHERE subject_id=$1::uuid ON CONFLICT DO NOTHING",
        )
        .bind(&subject)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM organic_discovery_unattributed_reads WHERE subject_id=$1::uuid",
        )
        .bind(&subject)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM rec_feedback_state WHERE subject_id=$1::uuid")
            .bind(&subject)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM rec_reader_signals WHERE subject_id=$1::uuid")
            .bind(&subject)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE rec_subjects SET subject_id=gen_random_uuid(),enabled=$2,revision=revision+1,feedback_epoch=feedback_epoch+CASE WHEN revision=0 THEN 0 ELSE 1 END,enabled_at=CASE WHEN $2 THEN now() ELSE NULL END,changed_at=now() WHERE user_id=$1")
            .bind(actor).bind(enabled).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM rec_preference_commands WHERE user_id=$1")
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO rec_preference_commands(user_id,command_key,payload_hash,result_revision,command_type) VALUES($1,$2,$3,$4,'consent')")
            .bind(actor).bind(key.as_slice()).bind(payload.as_slice()).bind(revision+1).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(ConsentState {
            enabled,
            revision: revision + 1,
        })
    }
}
