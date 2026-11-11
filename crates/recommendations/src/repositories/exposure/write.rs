use super::*;
use crate::{repositories::feedback::discovery::ACTOR_DISCOVERY_CTE, services::exposure::digest};

impl PgExposure {
    /// Bounded queue consumer only. Successful enqueue is not an opportunity until this commits.
    pub async fn register(&self, proofs: &[IssuedObservation]) -> Result<i64, ExposureError> {
        if proofs.is_empty() || proofs.len() > 64 || proofs.iter().any(|p| !p.card.valid()) {
            return Err(ExposureError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','500ms',true),set_config('lock_timeout','50ms',true)").execute(&mut *tx).await?;
        let live: i64 = sqlx::query_scalar(
            "SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if live + proofs.len() as i64 > MAX_PENDING_PROOFS {
            return Err(ExposureError::Capacity);
        }
        let hashes: Vec<Vec<u8>> = proofs.iter().map(|p| p.digest.to_vec()).collect();
        let books: Vec<i64> = proofs.iter().map(|p| p.card.book_id).collect();
        let surfaces: Vec<&str> = proofs.iter().map(|p| p.card.surface.as_str()).collect();
        let campaigns: Vec<i64> = proofs
            .iter()
            .map(|p| p.card.editorial.map_or(0, |e| e.campaign_id))
            .collect();
        let revisions: Vec<i64> = proofs
            .iter()
            .map(|p| p.card.editorial.map_or(0, |e| e.revision))
            .collect();
        let slots: Vec<&str> = proofs
            .iter()
            .map(|p| p.card.editorial.map_or("", |e| e.slot.as_str()))
            .collect();
        let registered:i64=sqlx::query_scalar(r#"WITH stamp AS MATERIALIZED (
         SELECT clock_timestamp() AS issued_at
        ), added AS (
         INSERT INTO flow_observation_proofs(digest,book_id,surface,campaign_id,campaign_revision,slot,issued_at,expires_at,bucket)
         SELECT x.*,s.issued_at,s.issued_at+interval '15 minutes',date_trunc('hour',s.issued_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
         FROM unnest($1::bytea[],$2::bigint[],$3::text[],$4::bigint[],$5::bigint[],$6::text[])
           x(digest,book_id,surface,campaign_id,campaign_revision,slot)
         CROSS JOIN stamp s
         WHERE EXISTS(SELECT 1 FROM rec_materializable_books b WHERE b.id=x.book_id)
         ON CONFLICT(digest) DO NOTHING RETURNING bucket,book_id,surface,campaign_id,slot
        ), bumped AS (
         INSERT INTO flow_observation_hourly(bucket,book_id,surface,campaign_id,slot,opportunities)
         SELECT bucket,book_id,surface,campaign_id,slot,count(*) FROM added GROUP BY 1,2,3,4,5
         ON CONFLICT(bucket,book_id,surface,campaign_id,slot) DO UPDATE
          SET opportunities=flow_observation_hourly.opportunities+EXCLUDED.opportunities
        ) SELECT count(*) FROM added"#)
            .bind(hashes).bind(books).bind(surfaces).bind(campaigns).bind(revisions).bind(slots)
            .fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE flow_observation_state SET live_proofs=live_proofs+$1 WHERE singleton")
            .bind(registered)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(registered)
    }

    /// Only the trusted server metadata is exposed to the integration-layer live guard.
    pub async fn describe(&self, proof: &str) -> Result<ProofContext, ExposureError> {
        let hash = digest(proof)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','500ms',true),set_config('lock_timeout','50ms',true)").execute(&mut *tx).await?;
        let row=sqlx::query("SELECT book_id,surface,campaign_id,campaign_revision,slot,expires_at>clock_timestamp() AS fresh FROM flow_observation_proofs WHERE digest=$1")
            .bind(hash.as_slice()).fetch_optional(&mut *tx).await?.ok_or(ExposureError::Missing)?;
        if !row.try_get::<bool, _>("fresh")? {
            return Err(ExposureError::Expired);
        }
        let context = ProofContext { card: card(&row)? };
        tx.commit().await?;
        Ok(context)
    }

    /// One transaction deduplicates each event bit and advances exactly its aggregate.
    /// Actor is an ephemeral authorization input. It is never stored in proof/aggregate rows.
    pub async fn record(
        &self,
        event: &ObservationEvent,
        context: &ProofContext,
        actor: Option<i64>,
    ) -> Result<ObservationReceipt, ExposureError> {
        event.validate()?;
        let hash = digest(&event.proof)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','500ms',true),set_config('lock_timeout','50ms',true)").execute(&mut *tx).await?;
        let row=sqlx::query("SELECT *,expires_at>clock_timestamp() AS fresh,issued_at<=clock_timestamp()-interval '1 second' AS old_enough FROM flow_observation_proofs WHERE digest=$1 FOR UPDATE")
            .bind(hash.as_slice()).fetch_optional(&mut *tx).await?.ok_or(ExposureError::Missing)?;
        if !row.try_get::<bool, _>("fresh")? {
            return Err(ExposureError::Expired);
        }
        if card(&row)? != context.card {
            return Err(ExposureError::Forbidden);
        }
        if event.kind == EventKind::View && !row.try_get::<bool, _>("old_enough")? {
            return Err(ExposureError::TooEarly);
        }
        let allowed:bool=sqlx::query_scalar(&format!("{ACTOR_DISCOVERY_CTE} SELECT EXISTS(SELECT 1 FROM rec_actor_books b WHERE id=$2 AND NOT EXISTS(SELECT 1 FROM rec_book_preferences p WHERE p.user_id=$1 AND p.book_id=b.id AND p.hidden AND p.expires_at>now()))"))
            .bind(actor).bind(context.card.book_id).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(ExposureError::Forbidden);
        }
        let rendered: bool = row.try_get("rendered")?;
        let viewed: bool = row.try_get("viewed")?;
        let clicked: bool = row.try_get("clicked")?;
        let replay = match event.kind {
            EventKind::Rendered => rendered,
            EventKind::View => viewed,
            EventKind::Click => clicked,
        };
        let render_delta = i64::from(!rendered);
        let view_delta = i64::from(event.kind == EventKind::View && !viewed);
        let click_delta = i64::from(event.kind == EventKind::Click && !clicked);
        sqlx::query("UPDATE flow_observation_proofs SET rendered=true,viewed=viewed OR $2,clicked=clicked OR $3 WHERE digest=$1")
            .bind(hash.as_slice()).bind(event.kind==EventKind::View).bind(event.kind==EventKind::Click).execute(&mut *tx).await?;
        sqlx::query("UPDATE flow_observation_hourly h SET rendered=h.rendered+$2,views=h.views+$3,clicks=h.clicks+$4,replayed=h.replayed+$5 FROM flow_observation_proofs p WHERE p.digest=$1 AND (h.bucket,h.book_id,h.surface,h.campaign_id,h.slot)=(p.bucket,p.book_id,p.surface,p.campaign_id,p.slot)")
            .bind(hash.as_slice()).bind(render_delta).bind(view_delta).bind(click_delta).bind(i64::from(replay)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(if replay {
            ObservationReceipt::Replay
        } else {
            ObservationReceipt::Recorded
        })
    }

    /// Bounded TTL cleanup, safe to call after restart and without any incoming events.
    pub async fn prune(&self) -> Result<u64, ExposureError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','500ms',true),set_config('lock_timeout','50ms',true)").execute(&mut *tx).await?;
        sqlx::query("SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        let removed:i64=sqlx::query_scalar(r#"WITH expired AS (
          SELECT digest FROM flow_observation_proofs WHERE expires_at<=clock_timestamp()
          ORDER BY expires_at,digest LIMIT 1000 FOR UPDATE SKIP LOCKED
        ), deleted AS (
          DELETE FROM flow_observation_proofs p USING expired e WHERE p.digest=e.digest
          RETURNING p.bucket,p.book_id,p.surface,p.campaign_id,p.slot,p.viewed
        ), bumps AS (
          SELECT bucket,book_id,surface,campaign_id,slot,count(*) FILTER(WHERE NOT viewed) AS missed
          FROM deleted GROUP BY 1,2,3,4,5
        ), updated AS (
          UPDATE flow_observation_hourly h SET expired_unobserved=h.expired_unobserved+b.missed
          FROM bumps b WHERE (h.bucket,h.book_id,h.surface,h.campaign_id,h.slot)=(b.bucket,b.book_id,b.surface,b.campaign_id,b.slot)
        ) SELECT count(*) FROM deleted"#).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE flow_observation_state SET live_proofs=live_proofs-$1 WHERE singleton")
            .bind(removed)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM flow_observation_hourly WHERE (bucket,book_id,surface,campaign_id,slot) IN (SELECT bucket,book_id,surface,campaign_id,slot FROM flow_observation_hourly WHERE bucket<now()-interval '181 days' ORDER BY bucket LIMIT 1000)").execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(removed as u64)
    }
}
