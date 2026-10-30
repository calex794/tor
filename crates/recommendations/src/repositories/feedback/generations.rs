//! Immutable completed generations, bounded build pages, CAS activation and rollback.
use super::*;
use crate::services::feedback::validate_revision;
use sha2::{Digest,Sha256};
use sqlx::Row;

impl PgRecommendations {
    /// Operator-authorized scheduling; no live generation is discarded on failure.
    pub async fn request_refresh(&self) -> Result<String,FeedbackError> {
        let mut tx=self.pool.begin().await?;
        sqlx::query("SELECT singleton FROM rec_runtime_policy WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx).await?;
        let old:Option<String>=sqlx::query_scalar("SELECT id::text FROM rec_generations WHERE status='building'")
            .fetch_optional(&mut *tx).await?;
        let id=match old {
            Some(id)=>id,
            None=>sqlx::query_scalar("INSERT INTO rec_generations DEFAULT VALUES RETURNING id::text")
                .fetch_one(&mut *tx).await?,
        };
        tx.commit().await?;
        Ok(id)
    }

    /// At most eight source books per lease/transaction. Another worker skips it.
    pub async fn advance_generation(&self) -> Result<bool,FeedbackError> {
        let mut tx=self.pool.begin().await?;
        let row=sqlx::query("SELECT id::text,cursor_book_id FROM rec_generations WHERE status='building' ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED")
            .fetch_optional(&mut *tx).await?;
        let Some(row)=row else {return Ok(false)};
        let id:String=row.try_get("id")?;
        let cursor:i64=row.try_get("cursor_book_id")?;
        if cursor==0 {
            if let Err(error)=super::sources::global_sources(&mut tx,&id).await {
                // SQL errors abort the transaction and leave the prior pointer safe.
                // Resource-limit rejection is a deliberate terminal build result.
                if error==FeedbackError::Limited {
                    sqlx::query("UPDATE rec_generations SET status='rejected',finished_at=now() WHERE id=$1::uuid")
                        .bind(&id).execute(&mut *tx).await?;
                    tx.commit().await?;
                }
                return Err(error);
            }
        }
        let books:Vec<i64>=sqlx::query_scalar("SELECT id FROM rec_public_books WHERE id>$1 ORDER BY id LIMIT 8")
            .bind(cursor).fetch_all(&mut *tx).await?;
        for book in &books {super::sources::book_sources(&mut tx,&id,*book).await?;}
        if let Some(last)=books.last() {
            sqlx::query("UPDATE rec_generations SET cursor_book_id=$2 WHERE id=$1::uuid")
                .bind(&id).bind(last).execute(&mut *tx).await?;
        }
        if books.len()<8 {
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM rec_candidate_edges WHERE generation_id=$1::uuid")
                .bind(&id).fetch_one(&mut *tx).await?;
            let manifest:[u8;32]=Sha256::digest(format!("rec13-build-v1:{id}:{count}:{}",books.last().copied().unwrap_or(cursor))).into();
            sqlx::query("UPDATE rec_generations SET status='validated',finished_at=now(),manifest_hash=$2 WHERE id=$1::uuid")
                .bind(&id).bind(manifest.as_slice()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn runtime_status(&self) -> Result<(Option<String>,Option<String>,i64,i16,bool),FeedbackError> {
        let r=sqlx::query("SELECT active_generation::text,previous_generation::text,revision,rollout_percent,hold FROM rec_runtime_policy WHERE singleton")
            .fetch_one(&self.pool).await?;
        Ok((r.try_get("active_generation")?,r.try_get("previous_generation")?,r.try_get("revision")?,r.try_get("rollout_percent")?,r.try_get("hold")?))
    }

    pub async fn activate_generation(&self, generation: &str, expected:i64, percent:i16) -> Result<i64,FeedbackError> {
        validate_revision(expected)?;
        if generation.len()!=36 || !generation.bytes().all(|b| b.is_ascii_hexdigit()||b==b'-') || ![0,1,5,10,25,50,100].contains(&percent) {return Err(FeedbackError::Invalid);}
        let changed=sqlx::query("UPDATE rec_runtime_policy SET previous_generation=CASE WHEN active_generation IS DISTINCT FROM $1::uuid THEN active_generation ELSE previous_generation END,active_generation=$1::uuid,rollout_percent=$3,hold=false,revision=revision+1 WHERE singleton AND revision=$2 AND EXISTS(SELECT 1 FROM rec_generations WHERE id=$1::uuid AND status='validated' AND finished_at IS NOT NULL AND manifest_hash IS NOT NULL)")
            .bind(generation).bind(expected).bind(percent).execute(&self.pool).await?;
        if changed.rows_affected()!=1 {return Err(FeedbackError::Conflict);}
        Ok(expected+1)
    }

    pub async fn hold(&self, expected:i64) -> Result<i64,FeedbackError> {
        validate_revision(expected)?;
        let changed=sqlx::query("UPDATE rec_runtime_policy SET hold=true,revision=revision+1 WHERE singleton AND revision=$1")
            .bind(expected).execute(&self.pool).await?;
        if changed.rows_affected()!=1 {return Err(FeedbackError::Conflict);}
        Ok(expected+1)
    }

    pub async fn rollback(&self, expected:i64) -> Result<i64,FeedbackError> {
        validate_revision(expected)?;
        let changed=sqlx::query("UPDATE rec_runtime_policy SET active_generation=previous_generation,previous_generation=active_generation,hold=false,revision=revision+1 WHERE singleton AND revision=$1 AND EXISTS(SELECT 1 FROM rec_generations WHERE id=previous_generation AND status='validated')")
            .bind(expected).execute(&self.pool).await?;
        if changed.rows_affected()!=1 {return Err(FeedbackError::Conflict);}
        Ok(expected+1)
    }
}
