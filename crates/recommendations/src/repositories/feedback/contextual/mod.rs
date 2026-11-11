//! Explicit version boundary over the existing immutable generation store.
use super::*;
use crate::services::contextual::{Action, Control, Generation, Receipt, Status, MODEL};
use sqlx::Row;
mod build;
mod serving;

impl PgRecommendations {
    pub async fn contextual_status(&self) -> Result<Status, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        let policy=sqlx::query("SELECT active_generation::text,previous_generation::text,revision,hold,rollout_percent FROM rec_runtime_policy WHERE singleton")
            .fetch_one(&mut *tx).await?;
        let rows=sqlx::query("SELECT id::text,status,cursor_book_id,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,to_char(finished_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS finished_at FROM rec_generations WHERE model_version=$1 ORDER BY created_at DESC,id DESC LIMIT 20")
            .bind(MODEL).fetch_all(&mut *tx).await?;
        let generations = rows
            .into_iter()
            .map(|r| {
                Ok(Generation {
                    id: r.try_get("id")?,
                    status: r.try_get("status")?,
                    cursor_book_id: r.try_get::<i64, _>("cursor_book_id")?.to_string(),
                    created_at: r.try_get("created_at")?,
                    finished_at: r.try_get("finished_at")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let result = Status {
            model_version: MODEL,
            revision: policy.try_get("revision")?,
            active_generation: policy.try_get("active_generation")?,
            previous_generation: policy.try_get("previous_generation")?,
            held: policy.try_get::<bool, _>("hold")?
                || policy.try_get::<i16, _>("rollout_percent")? != 100,
            generations,
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn control_contextual(&self, command: &Control) -> Result<Receipt, FeedbackError> {
        command.validate()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        let row=sqlx::query("SELECT active_generation::text,previous_generation::text,revision FROM rec_runtime_policy WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx).await?;
        let revision: i64 = row.try_get("revision")?;
        if revision != command.expected_revision {
            return Err(FeedbackError::Conflict);
        }
        let active: Option<String> = row.try_get("active_generation")?;
        let previous: Option<String> = row.try_get("previous_generation")?;
        let generation = match command.action {
            Action::Refresh => {
                let current:Option<String>=sqlx::query_scalar("SELECT id::text FROM rec_generations WHERE model_version=$1 AND status='building'")
                    .bind(MODEL).fetch_optional(&mut *tx).await?;
                let id = match current {
                    Some(id) => id,
                    None => sqlx::query_scalar(
                        "INSERT INTO rec_generations(model_version) VALUES($1) RETURNING id::text",
                    )
                    .bind(MODEL)
                    .fetch_one(&mut *tx)
                    .await?,
                };
                Some(id)
            }
            Action::Activate | Action::Rollback => {
                let chosen = if command.action == Action::Activate {
                    command.generation.as_ref()
                } else {
                    previous.as_ref()
                }
                .ok_or(FeedbackError::Conflict)?;
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_generations WHERE id=$1::uuid AND model_version=$2 AND status='validated' AND finished_at IS NOT NULL AND octet_length(manifest_hash)=32)")
                    .bind(chosen).bind(MODEL).fetch_one(&mut *tx).await?;
                if !valid {
                    return Err(FeedbackError::Conflict);
                }
                let previous = if active.as_ref() == Some(chosen) {
                    previous.as_ref()
                } else {
                    active.as_ref()
                };
                sqlx::query("UPDATE rec_runtime_policy SET active_generation=$1::uuid,previous_generation=$2::uuid,hold=false,rollout_percent=100 WHERE singleton")
                    .bind(chosen).bind(previous).execute(&mut *tx).await?;
                Some(chosen.clone())
            }
            Action::Cancel => {
                let changed=sqlx::query("UPDATE rec_generations SET status='rejected',finished_at=now() WHERE id=$1::uuid AND model_version=$2 AND status='building'")
                    .bind(&command.generation).bind(MODEL).execute(&mut *tx).await?.rows_affected();
                if changed != 1 {
                    return Err(FeedbackError::Conflict);
                }
                command.generation.clone()
            }
            Action::Hold => {
                sqlx::query("UPDATE rec_runtime_policy SET hold=true WHERE singleton")
                    .execute(&mut *tx)
                    .await?;
                active
            }
        };
        sqlx::query("UPDATE rec_runtime_policy SET revision=revision+1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Receipt {
            revision: revision + 1,
            generation,
        })
    }
}
