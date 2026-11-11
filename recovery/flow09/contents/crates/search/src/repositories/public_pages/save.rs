use super::*;

impl PgPublicPages {
    pub async fn save(
        &self,
        lease: &PageProbeLease,
        results: &[PageProbeResult],
    ) -> Result<(), PageProbeError> {
        if lease.targets.len()!=4 || results.len()!=4 || results.iter().zip(&lease.targets).any(|(r,t)|
            r.target!=*t || r.status.is_some_and(|code| !(100..=599).contains(&code))) {
            return Err(PageProbeError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        limits(&mut tx, false).await?;
        let previous_origin: Option<String> = sqlx::query_scalar("SELECT finished_origin FROM flow_public_page_probe
            WHERE singleton AND epoch=$1 AND origin=$2 AND lease_until>=clock_timestamp() FOR UPDATE")
            .bind(lease.epoch).bind(&lease.origin).fetch_optional(&mut *tx).await.map_err(unavailable)?
            .ok_or(PageProbeError::Stale)?;
        // Brief SHARE locks hold current B/C revisions stable until snapshot commit.
        // These locks are acquired only after all network operations have finished.
        let live = candidates::read(&mut tx, true).await?;
        let old = sqlx::query("SELECT *,checked_at>=now()-interval '180 seconds' AS fresh
            FROM flow_public_page_results ORDER BY slot LIMIT 4")
            .fetch_all(&mut *tx).await.map_err(unavailable)?;
        for candidate in live {
            let result = results.iter().find(|r| r.target == candidate);
            let status = result.and_then(|r| r.status).filter(|_| candidate.path().is_some());
            let previous = old.iter().find(|r| target(r).as_ref()==Ok(&candidate));
            let continuing = previous_origin.as_deref()==Some(&lease.origin)
                && previous.is_some_and(|r| r.try_get::<bool,_>("fresh").unwrap_or(false));
            let baseline = status==Some(200) || continuing && previous.is_some_and(|r|
                r.try_get::<bool,_>("baseline_known").unwrap_or(false));
            sqlx::query("INSERT INTO flow_public_page_results(slot,book_id,chapter_id,book_revision,
                chapter_revision,publication_revision,version_id,http_status,baseline_known,last_success_at,checked_at)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,
                    CASE WHEN $8=200 THEN now() WHEN $10 THEN (SELECT last_success_at FROM flow_public_page_results WHERE slot=$1) END,now())
                ON CONFLICT(slot) DO UPDATE SET book_id=EXCLUDED.book_id,chapter_id=EXCLUDED.chapter_id,
                book_revision=EXCLUDED.book_revision,chapter_revision=EXCLUDED.chapter_revision,
                publication_revision=EXCLUDED.publication_revision,version_id=EXCLUDED.version_id,
                http_status=EXCLUDED.http_status,baseline_known=EXCLUDED.baseline_known,
                last_success_at=EXCLUDED.last_success_at,checked_at=EXCLUDED.checked_at")
                .bind(candidate.slot.code()).bind(candidate.book_id).bind(candidate.chapter_id)
                .bind(&candidate.book_revision).bind(&candidate.chapter_revision).bind(&candidate.publication_revision)
                .bind(candidate.version_id).bind(status.map(|s| s as i16)).bind(baseline).bind(continuing)
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        let changed=sqlx::query("UPDATE flow_public_page_probe SET finished_at=now(),finished_origin=origin,lease_until=NULL
            WHERE singleton AND epoch=$1 AND lease_until>=clock_timestamp()")
            .bind(lease.epoch).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        if changed!=1 { return Err(PageProbeError::Stale); }
        tx.commit().await.map_err(unavailable)?;
        Ok(())
    }
}
