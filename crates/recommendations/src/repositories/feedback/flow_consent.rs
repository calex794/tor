//! Explicit, independently revocable FLOW scope for registered REC subjects.
use super::*;
use crate::{
    domain::feedback::{token_digest, ConsentState},
    services::{
        feedback::{validate_actor, validate_revision},
        visits::FlowScope,
    },
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgRecommendations {
    pub async fn flow_consent(&self, actor: i64) -> Result<ConsentState, FeedbackError> {
        validate_actor(actor)?;
        let row = sqlx::query(
            "SELECT flow_epoch IS NOT NULL AS enabled,revision
             FROM rec_subjects WHERE user_id=$1",
        )
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

    /// An internal capability, never an identifier returned to a browser.
    /// The durable observer rechecks this epoch while holding the subject lock.
    pub async fn flow_scope(&self, actor: i64) -> Result<Option<FlowScope>, FeedbackError> {
        validate_actor(actor)?;
        let row = sqlx::query(
            "SELECT s.subject_id::text,s.flow_epoch::text
             FROM rec_subjects s JOIN users u ON u.id=s.user_id
             WHERE s.user_id=$1 AND s.enabled AND s.flow_epoch IS NOT NULL
               AND u.account_state='active'",
        )
        .bind(actor)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(FlowScope {
                subject_id: row.try_get("subject_id")?,
                epoch: row.try_get("flow_epoch")?,
            })
        })
        .transpose()
    }

    /// Uses the existing preference CAS and replay ledger without treating an
    /// earlier REC opt-in as permission to collect durable FLOW visits.
    pub async fn set_flow_consent(
        &self,
        actor: i64,
        enabled: bool,
        revision: i64,
        command: &str,
    ) -> Result<ConsentState, FeedbackError> {
        validate_actor(actor)?;
        validate_revision(revision)?;
        let key = token_digest(command)?;
        let payload: [u8; 32] = Sha256::digest(format!("flow-consent:{enabled}:{revision}")).into();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO rec_subjects(user_id)
             SELECT id FROM users WHERE id=$1 AND account_state='active'
             ON CONFLICT DO NOTHING",
        )
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        let state = sqlx::query(
            "SELECT s.subject_id::text,s.revision,s.enabled,s.flow_epoch::text
             FROM rec_subjects s JOIN users u ON u.id=s.user_id
             WHERE s.user_id=$1 AND u.account_state='active'
             FOR UPDATE OF s FOR SHARE OF u",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(FeedbackError::Forbidden)?;
        if let Some(receipt) = sqlx::query(
            "SELECT payload_hash,result_revision,command_type
             FROM rec_preference_commands WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(key.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            if receipt.try_get::<Vec<u8>, _>("payload_hash")? != payload
                || receipt.try_get::<String, _>("command_type")? != "consent"
            {
                return Err(FeedbackError::IdempotencyConflict);
            }
            return Ok(ConsentState {
                enabled,
                revision: receipt.try_get("result_revision")?,
            });
        }
        if state.try_get::<i64, _>("revision")? != revision {
            return Err(FeedbackError::Conflict);
        }
        if enabled && !state.try_get::<bool, _>("enabled")? {
            return Err(FeedbackError::Forbidden);
        }
        let old_epoch: Option<String> = state.try_get("flow_epoch")?;
        if !enabled {
            if let Some(epoch) = old_epoch.as_deref() {
                let subject: String = state.try_get("subject_id")?;
                sqlx::query("SELECT flow_erase_scope($1::uuid,$2::uuid)")
                    .bind(subject)
                    .bind(epoch)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        // A repeated true/false command advances the CAS, as REC consent does,
        // but only a real off -> on transition creates a new collection epoch.
        sqlx::query(
            "UPDATE rec_subjects SET
               flow_epoch=CASE WHEN $2 THEN COALESCE(flow_epoch,gen_random_uuid()) ELSE NULL END,
               flow_enabled_at=CASE WHEN $2 THEN COALESCE(flow_enabled_at,now()) ELSE NULL END,
               revision=revision+1,changed_at=now() WHERE user_id=$1",
        )
        .bind(actor)
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO rec_preference_commands
               (user_id,command_key,payload_hash,result_revision,command_type)
             VALUES($1,$2,$3,$4,'consent')",
        )
        .bind(actor)
        .bind(key.as_slice())
        .bind(payload.as_slice())
        .bind(revision + 1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ConsentState {
            enabled,
            revision: revision + 1,
        })
    }
}
