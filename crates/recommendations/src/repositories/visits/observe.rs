use super::{unavailable, PgFlowVisits};
use crate::services::visits::{
    FlowVisitError, VisitObservation, FLOW_VISIT_FUTURE_SECONDS, FLOW_VISIT_IDLE_SECONDS,
    FLOW_VISIT_MAX_DELAY_SECONDS,
};
use sqlx::{Postgres, Row, Transaction};

impl PgFlowVisits {
    /// A receipt and the compact activity window commit together. Consent is
    /// checked again here, after any time spent in the bounded web queue.
    /// False means replay or a no-longer-eligible observation, never a new fact.
    pub async fn observe(&self, observation: &VisitObservation) -> Result<bool, FlowVisitError> {
        observation.validate()?;
        let at = observation.observed_at_millis;
        let idle = FLOW_VISIT_IDLE_SECONDS * 1_000;
        let earliest = at.checked_sub(idle).ok_or(FlowVisitError::Invalid)?;
        let latest = at.checked_add(idle).ok_or(FlowVisitError::Invalid)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        // All capture, erasure and finalization use subject -> visit/daily.
        // No expired epoch can be resurrected by a late queued observation.
        let eligible: Option<String> = sqlx::query_scalar(
            "SELECT s.subject_id::text FROM rec_subjects s
             JOIN users u ON u.id=s.user_id AND u.account_state='active'
             WHERE s.subject_id=$1::uuid AND s.flow_epoch=$2::uuid AND s.enabled
               AND to_timestamp($3::double precision/1000)>=date_trunc('milliseconds',s.flow_enabled_at)
               AND to_timestamp($3::double precision/1000)>=clock_timestamp()-make_interval(secs=>$4)
               AND to_timestamp($3::double precision/1000)<=clock_timestamp()+make_interval(secs=>$5)
             FOR UPDATE OF s FOR SHARE OF u",
        )
        .bind(&observation.scope.subject_id)
        .bind(&observation.scope.epoch)
        .bind(at)
        .bind(FLOW_VISIT_MAX_DELAY_SECONDS as f64)
        .bind(FLOW_VISIT_FUTURE_SECONDS as f64)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if eligible.is_none() {
            return Ok(false);
        }
        let receipt = sqlx::query(
            "INSERT INTO flow_visit_receipts
               (event_id,subject_id,flow_epoch,observed_at,surface,acquisition_hint)
             VALUES($1::uuid,$2::uuid,$3::uuid,to_timestamp($4::double precision/1000),$5,$6)
             ON CONFLICT(event_id) DO NOTHING",
        )
        .bind(&observation.event_id)
        .bind(&observation.scope.subject_id)
        .bind(&observation.scope.epoch)
        .bind(at)
        .bind(observation.surface)
        .bind(observation.acquisition_hint)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if receipt.rows_affected() == 0 {
            let same: bool = sqlx::query_scalar(
                "SELECT subject_id=$2::uuid AND flow_epoch=$3::uuid
                        AND observed_at=to_timestamp($4::double precision/1000)
                        AND surface=$5 AND acquisition_hint=$6
                 FROM flow_visit_receipts WHERE event_id=$1::uuid",
            )
            .bind(&observation.event_id)
            .bind(&observation.scope.subject_id)
            .bind(&observation.scope.epoch)
            .bind(at)
            .bind(observation.surface)
            .bind(observation.acquisition_hint)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            return if same {
                Ok(false)
            } else {
                Err(FlowVisitError::Conflict)
            };
        }
        let continuing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM flow_visit_continuations
               WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
                 AND last_at>to_timestamp($3::double precision/1000))",
        )
        .bind(&observation.scope.subject_id)
        .bind(&observation.scope.epoch)
        .bind(earliest)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if continuing {
            // The first thirty days are already anonymous. Never manufacture
            // a new start or retain its old source beyond the privacy limit.
            // A late bridge may also retract a recent apparent new start.
            continue_erased_visit(&mut tx, observation, earliest, latest).await?;
            record_account_day(&mut tx, observation).await?;
            tx.commit().await.map_err(unavailable)?;
            return Ok(true);
        }
        let adjacent = sqlx::query(
            "SELECT id::text,first_event_id::text,last_event_id::text,
                    round(extract(epoch FROM started_at)*1000)::bigint AS first_millis,
                    round(extract(epoch FROM last_at)*1000)::bigint AS last_millis,
                    landing_surface,last_surface,acquisition_hint,pageviews
             FROM flow_visits WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
               AND finalized_at IS NULL
               AND started_at<to_timestamp($3::double precision/1000)
               AND last_at>to_timestamp($4::double precision/1000)
             ORDER BY started_at,first_event_id FOR UPDATE",
        )
        .bind(&observation.scope.subject_id)
        .bind(&observation.scope.epoch)
        .bind(latest)
        .bind(earliest)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        let event = observation.event_id.to_ascii_lowercase();
        let mut first = (at, event.clone());
        let mut last = first.clone();
        let mut landing = observation.surface;
        let mut exit = observation.surface;
        let mut source = observation.acquisition_hint;
        let mut pageviews = 1_i64;
        let mut id = event.clone();
        for row in &adjacent {
            let row_first = (
                row.try_get::<i64, _>("first_millis").map_err(unavailable)?,
                row.try_get::<String, _>("first_event_id")
                    .map_err(unavailable)?,
            );
            let row_last = (
                row.try_get::<i64, _>("last_millis").map_err(unavailable)?,
                row.try_get::<String, _>("last_event_id")
                    .map_err(unavailable)?,
            );
            if row_first < first {
                first = row_first;
                landing = row.try_get("landing_surface").map_err(unavailable)?;
                source = row.try_get("acquisition_hint").map_err(unavailable)?;
            }
            if row_last > last {
                last = row_last;
                exit = row.try_get("last_surface").map_err(unavailable)?;
            }
            pageviews = pageviews
                .checked_add(row.try_get::<i64, _>("pageviews").map_err(unavailable)?)
                .ok_or(FlowVisitError::Unavailable)?;
        }
        if let Some(first_row) = adjacent.first() {
            id = first_row.try_get("id").map_err(unavailable)?;
            // An out-of-order event may bridge two previously separate windows.
            // Their receipts remain, while the visit count becomes one.
            sqlx::query(
                "DELETE FROM flow_visits WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
                   AND finalized_at IS NULL AND id<>$3::uuid
                   AND started_at<to_timestamp($4::double precision/1000)
                   AND last_at>to_timestamp($5::double precision/1000)",
            )
            .bind(&observation.scope.subject_id)
            .bind(&observation.scope.epoch)
            .bind(&id)
            .bind(latest)
            .bind(earliest)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        sqlx::query(
            "INSERT INTO flow_visits
               (id,subject_id,flow_epoch,started_at,last_at,landing_surface,last_surface,
                acquisition_hint,pageviews,first_event_id,last_event_id)
             VALUES($1::uuid,$2::uuid,$3::uuid,to_timestamp($4::double precision/1000),
                    to_timestamp($5::double precision/1000),$6,$7,$8,$9,$10::uuid,$11::uuid)
             ON CONFLICT(id) DO UPDATE SET
               started_at=EXCLUDED.started_at,last_at=EXCLUDED.last_at,
               landing_surface=EXCLUDED.landing_surface,last_surface=EXCLUDED.last_surface,
               acquisition_hint=EXCLUDED.acquisition_hint,pageviews=EXCLUDED.pageviews,
               first_event_id=EXCLUDED.first_event_id,last_event_id=EXCLUDED.last_event_id",
        )
        .bind(id)
        .bind(&observation.scope.subject_id)
        .bind(&observation.scope.epoch)
        .bind(first.0)
        .bind(last.0)
        .bind(landing)
        .bind(exit)
        .bind(source)
        .bind(pageviews)
        .bind(first.1)
        .bind(last.1)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        record_account_day(&mut tx, observation).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(true)
    }
}

async fn record_account_day(
    tx: &mut Transaction<'_, Postgres>,
    observation: &VisitObservation,
) -> Result<(), FlowVisitError> {
    sqlx::query(
        "INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
         VALUES($1::uuid,$2::uuid,(to_timestamp($3::double precision/1000) AT TIME ZONE 'UTC')::date)
         ON CONFLICT DO NOTHING",
    )
    .bind(&observation.scope.subject_id)
    .bind(&observation.scope.epoch)
    .bind(observation.observed_at_millis)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}

async fn continue_erased_visit(
    tx: &mut Transaction<'_, Postgres>,
    observation: &VisitObservation,
    earliest: i64,
    latest: i64,
) -> Result<(), FlowVisitError> {
    sqlx::query(
        "WITH erased AS (
           DELETE FROM flow_visits WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
             AND finalized_at IS NULL
             AND started_at<to_timestamp($3::double precision/1000)
             AND last_at>to_timestamp($4::double precision/1000)
           RETURNING started_at,last_at
         ), censored AS (
           UPDATE flow_visit_receipts r SET continuation_unknown=true
           WHERE r.subject_id=$1::uuid AND r.flow_epoch=$2::uuid
             AND (r.event_id=$5::uuid OR EXISTS(SELECT 1 FROM erased e
                  WHERE r.observed_at>=e.started_at AND r.observed_at<=e.last_at))
         )
         UPDATE flow_visit_continuations SET last_at=GREATEST(last_at,
             to_timestamp($6::double precision/1000),
             COALESCE((SELECT max(last_at) FROM erased),last_at))
         WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid",
    )
    .bind(&observation.scope.subject_id)
    .bind(&observation.scope.epoch)
    .bind(latest)
    .bind(earliest)
    .bind(&observation.event_id)
    .bind(observation.observed_at_millis)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}
