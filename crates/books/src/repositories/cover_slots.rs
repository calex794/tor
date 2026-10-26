//! B-owned cover-slot transition. G never updates book_cover_slots.
//! A single PG commit verifies actor, staged main/card pair and revision CAS.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCoverSlots {
    pool: PgPool,
}
impl PgCoverSlots {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn current_slot(&self, book_id: i64) -> Result<Option<(i64, i64)>, sqlx::Error> {
        let row = sqlx::query("SELECT job_id,revision FROM book_cover_slots WHERE book_id=$1")
            .bind(book_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| Ok((row.try_get("job_id")?, row.try_get("revision")?)))
            .transpose()
    }

    /// Returns false if the actor changed, a later cover won, or B has a hold.
    /// Same job/current slot is an idempotent success after a worker crash.
    pub async fn attach_ready_job(
        &self,
        job_id: i64,
        lease_epoch: i64,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let job = sqlx::query(
            "SELECT j.book_id,j.expected_slot_revision FROM cover_variant_jobs j
             JOIN books b ON b.id=j.book_id
             WHERE j.id=$1 AND j.lease_epoch=$2
               AND j.status IN ('running','ready') AND j.locked_until>now()
               AND b.owner_user_id=j.actor_user_id
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (SELECT count(*) FROM cover_variants v WHERE v.job_id=j.id)=2
               AND EXISTS(SELECT 1 FROM cover_variants v WHERE v.job_id=j.id AND v.variant_name='main')
               AND EXISTS(SELECT 1 FROM cover_variants v WHERE v.job_id=j.id AND v.variant_name='card')
             FOR UPDATE OF b,j",
        ).bind(job_id).bind(lease_epoch).fetch_optional(&mut *tx).await?;
        let Some(job) = job else {
            return Ok(false);
        };
        let book_id: i64 = job.try_get("book_id")?;
        let expected: i64 = job.try_get("expected_slot_revision")?;
        let slot =
            sqlx::query("SELECT job_id,revision FROM book_cover_slots WHERE book_id=$1 FOR UPDATE")
                .bind(book_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(slot) = slot {
            if slot.try_get::<i64, _>("job_id")? == job_id {
                tx.commit().await?;
                return Ok(true);
            }
            if slot.try_get::<i64, _>("revision")? != expected {
                return Ok(false);
            }
        } else if expected != 0 {
            return Ok(false);
        }
        let changed = sqlx::query(
            "INSERT INTO book_cover_slots(book_id,job_id,revision)
             VALUES($1,$2,1)
             ON CONFLICT(book_id) DO UPDATE
               SET job_id=EXCLUDED.job_id,revision=book_cover_slots.revision+1,
                   attached_at=now()
               WHERE book_cover_slots.revision=$3
             RETURNING revision",
        )
        .bind(book_id)
        .bind(job_id)
        .bind(expected)
        .fetch_optional(&mut *tx)
        .await?;
        if changed.is_none() {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }
}
