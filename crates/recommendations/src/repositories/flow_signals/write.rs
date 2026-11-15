use super::*;

impl PgFlowSignals {
    /// Claim before reading source owners. Another process cannot overwrite this
    /// snapshot with an earlier round, including after a crashed writer's lease.
    pub async fn claim_round(&self, days: i32) -> Result<Option<SignalLease>, SignalError> {
        if ![0, 1, 7, 30, 90].contains(&days) {
            return Err(SignalError::Invalid);
        }
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let row = sqlx::query(
                r#"WITH stamp AS MATERIALIZED (
 SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS ms
) UPDATE flow_signal_rounds r SET epoch=epoch+1,started_ms=s.ms,
 lease_until_ms=s.ms+15000,committed=false,payload_digest=NULL FROM stamp s
 WHERE days=$1 AND lease_until_ms<=s.ms
 RETURNING epoch,started_ms,lease_until_ms"#,
            )
            .bind(days)
            .fetch_optional(&mut *tx)
            .await?;
            let lease = row
                .map(|r| -> Result<_, SignalError> {
                    Ok(SignalLease {
                        days,
                        epoch: r.try_get("epoch")?,
                        started_ms: r.try_get("started_ms")?,
                        until_ms: r.try_get("lease_until_ms")?,
                    })
                })
                .transpose()?;
            tx.commit().await?;
            Ok(lease)
        })
        .await
    }

    pub async fn record_round(
        &self,
        lease: SignalLease,
        observations: &[SignalObservation],
    ) -> Result<(), SignalError> {
        let expected: Vec<_> = Rule::ALL
            .into_iter()
            .filter(|r| r.accepts_days(lease.days))
            .collect();
        if expected.is_empty()
            || observations.len() != expected.len()
            || expected
                .iter()
                .any(|r| observations.iter().filter(|o| &o.rule == r).count() != 1)
        {
            return Err(SignalError::Invalid);
        }
        for o in observations {
            if !valid_scope(&o.scope) {
                return Err(SignalError::Invalid);
            }
            o.window.validate()?;
            o.evidence.validate()?;
        }
        let mut digest_parts = observations
            .iter()
            .map(|o| {
                format!(
                    "{}:{}:{:?}:{:?}",
                    o.rule.code(),
                    o.scope,
                    o.window,
                    o.evidence
                )
            })
            .collect::<Vec<_>>();
        digest_parts.sort();
        let digest = source_scope(&digest_parts.iter().map(String::as_str).collect::<Vec<_>>());
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let row = sqlx::query(
                "SELECT *,lease_until_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS live FROM flow_signal_rounds WHERE days=$1 FOR UPDATE"
            ).bind(lease.days).fetch_one(&mut *tx).await?;
            if row.try_get::<i64, _>("epoch")? != lease.epoch
                || row.try_get::<i64, _>("started_ms")? != lease.started_ms
            {
                return Err(SignalError::Stale);
            }
            if row.try_get::<bool, _>("committed")? {
                let previous: Option<String> = row.try_get("payload_digest")?;
                return if previous.as_deref() == Some(digest.as_str()) { Ok(()) } else { Err(SignalError::Stale) };
            }
            if !row.try_get::<bool, _>("live")? { return Err(SignalError::Stale); }
            for o in observations {
                let old = sqlx::query("SELECT * FROM flow_signal_states WHERE rule=$1 AND days=$2 AND scope=$3 FOR UPDATE")
                    .bind(o.rule.code()).bind(lease.days).bind(&o.scope).fetch_optional(&mut *tx).await?;
                let prior = old.as_ref().map(episode).transpose()?.unwrap_or_default();
                let result = evaluate(o.rule, o.evidence)?;
                let next = transition(prior, result, lease.started_ms);
                write_state(&mut tx, lease, o, result, next.episode).await?;
                if let Some(kind) = next.event {
                    sqlx::query(&format!(
                        "INSERT INTO flow_signal_events ({COLUMNS},kind) SELECT {COLUMNS},$4 FROM flow_signal_states WHERE rule=$1 AND days=$2 AND scope=$3 ON CONFLICT(rule,days,scope,episode_seq,kind) DO NOTHING"
                    )).bind(o.rule.code()).bind(lease.days).bind(&o.scope).bind(kind.code()).execute(&mut *tx).await?;
                }
            }
            let changed = sqlx::query(
                "UPDATE flow_signal_rounds SET committed=true,payload_digest=$3,lease_until_ms=0 WHERE days=$1 AND epoch=$2 AND lease_until_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint"
            ).bind(lease.days).bind(lease.epoch).bind(digest).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 { return Err(SignalError::Stale); }
            tx.commit().await?;
            Ok(())
        }).await
    }

    /// Finite maintenance. Open episodes and unresolved source scopes survive.
    pub async fn prune(&self) -> Result<u64, SignalError> {
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let removed = sqlx::query(
                "DELETE FROM flow_signal_events WHERE id IN(SELECT id FROM flow_signal_events WHERE checked_at_ms<floor(extract(epoch FROM now()-interval '180 days')*1000)::bigint ORDER BY checked_at_ms,id LIMIT 500 FOR UPDATE SKIP LOCKED)"
            ).execute(&mut *tx).await?.rows_affected();
            sqlx::query(
                "DELETE FROM flow_signal_states WHERE (rule,days,scope) IN(SELECT rule,days,scope FROM flow_signal_states WHERE episode_reason IS NULL AND checked_at_ms<floor(extract(epoch FROM now()-interval '181 days')*1000)::bigint ORDER BY checked_at_ms LIMIT 500 FOR UPDATE SKIP LOCKED)"
            ).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(removed)
        }).await
    }
}

async fn write_state(
    tx: &mut Transaction<'_, Postgres>,
    lease: SignalLease,
    o: &SignalObservation,
    result: Evaluation,
    next: Episode,
) -> Result<(), SignalError> {
    let e = o.evidence;
    let mut q = sqlx::QueryBuilder::<Postgres>::new(format!(
        "INSERT INTO flow_signal_states ({COLUMNS}) VALUES("
    ));
    let mut values = q.separated(",");
    values
        .push_bind(o.rule.code())
        .push_bind(lease.days)
        .push_bind(&o.scope)
        .push_bind(result.status.code())
        .push_bind(result.reason.code())
        .push_bind(lease.epoch)
        .push_bind(lease.started_ms)
        .push_bind(o.window.until_epoch)
        .push_bind(&o.window.label)
        .push_bind(e.enabled)
        .push_bind(e.available)
        .push_bind(e.current)
        .push_bind(e.previous)
        .push_bind(e.current_units)
        .push_bind(e.previous_units)
        .push_bind(e.expected_units)
        .push_bind(e.known_discards)
        .push_bind(e.source_failed)
        .push_bind(e.limited_units)
        .push_bind(e.omitted_rows)
        .push_bind(e.current_ack_per_mille)
        .push_bind(e.previous_ack_per_mille)
        .push_bind(e.current_sample)
        .push_bind(e.previous_sample)
        .push_bind(e.current_opportunities)
        .push_bind(e.previous_opportunities)
        .push_bind(e.unknown_entities)
        .push_bind(next.sequence)
        .push_bind(next.reason.map(Reason::code))
        .push_bind(next.opened_ms)
        .push_bind(next.opening_emitted)
        .push_bind(next.last_notice_ms);
    q.push(") ON CONFLICT(rule,days,scope) DO UPDATE SET ");
    let mut updates = q.separated(",");
    for column in COLUMNS.split(',').skip(3) {
        updates.push(format!("{column}=EXCLUDED.{column}"));
    }
    q.build().execute(&mut **tx).await?;
    Ok(())
}
