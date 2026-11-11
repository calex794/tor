use super::*;

impl PgFlowSignals {
    pub async fn report(
        &self,
        days: i32,
        before: Option<i64>,
    ) -> Result<SignalReport, SignalError> {
        if ![1, 7, 30, 90].contains(&days) || before.is_some_and(|n| n <= 0) {
            return Err(SignalError::Invalid);
        }
        bounded(async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx).await?;
            settings(&mut tx).await?;
            let now: i64 = sqlx::query_scalar("SELECT floor(extract(epoch FROM now())*1000)::bigint")
                .fetch_one(&mut *tx).await?;
            let rows = sqlx::query(
                "SELECT DISTINCT ON(rule) * FROM flow_signal_states WHERE days IN(0,$1) ORDER BY rule,last_epoch DESC,checked_at_ms DESC,scope LIMIT 13"
            ).bind(days).fetch_all(&mut *tx).await?;
            let states = rows.iter().map(state).collect::<Result<_, _>>()?;
            let rows = sqlx::query(
                "SELECT * FROM flow_signal_events WHERE days IN(0,$1) AND ($2::bigint IS NULL OR id<$2) ORDER BY id DESC LIMIT 51"
            ).bind(days).bind(before).fetch_all(&mut *tx).await?;
            let mut events = Vec::new();
            for row in rows.iter().take(50) {
                let kind = match row.try_get::<&str, _>("kind")? {
                    "opened" => EventKind::Opened,
                    "recovered" => EventKind::Recovered,
                    _ => return Err(SignalError::Unavailable),
                };
                events.push(SignalEvent { id: row.try_get("id")?, kind, state: state(row)? });
            }
            let next = if rows.len() > 50 { events.last().map(|e| e.id) } else { None };
            tx.commit().await?;
            Ok(SignalReport { checked_at_ms: now, days, states, events, next })
        }).await
    }
}
fn state(row: &sqlx::postgres::PgRow) -> Result<SignalState, SignalError> {
    Ok(SignalState {
        rule: Rule::from_str(row.try_get("rule")?)?,
        days: row.try_get("days")?,
        scope: row.try_get("scope")?,
        window: SignalWindow {
            until_epoch: row.try_get("until_epoch")?,
            label: row.try_get("window_label")?,
        },
        result: Evaluation::new(
            Status::from_str(row.try_get("status")?)?,
            Reason::from_str(row.try_get("reason")?)?,
        ),
        evidence: Evidence {
            enabled: row.try_get("enabled")?,
            available: row.try_get("available")?,
            current: row.try_get("current_value")?,
            previous: row.try_get("previous_value")?,
            current_units: row.try_get("current_units")?,
            previous_units: row.try_get("previous_units")?,
            expected_units: row.try_get("expected_units")?,
            known_discards: row.try_get("known_discards")?,
            source_failed: row.try_get("source_failed")?,
            limited_units: row.try_get("limited_units")?,
            omitted_rows: row.try_get("omitted_rows")?,
            current_ack_per_mille: row.try_get("current_ack_per_mille")?,
            previous_ack_per_mille: row.try_get("previous_ack_per_mille")?,
            current_sample: row.try_get("current_sample")?,
            previous_sample: row.try_get("previous_sample")?,
            current_opportunities: row.try_get("current_opportunities")?,
            previous_opportunities: row.try_get("previous_opportunities")?,
            unknown_entities: row.try_get("unknown_entities")?,
        },
        episode: episode(row)?,
        checked_at_ms: row.try_get("checked_at_ms")?,
        last_epoch: row.try_get("last_epoch")?,
    })
}
