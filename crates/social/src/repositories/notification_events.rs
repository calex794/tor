//! Append social notification facts to the existing I-owned outbox inside the source TX.
//! The consumer is independently idempotent; a retry never creates another source fact.
use super::mail::unavailable;
use crate::domain::MailError;
use sqlx::{Postgres, Transaction};

pub(super) async fn queue(
    tx: &mut Transaction<'_, Postgres>,
    event_type: &'static str,
    aggregate_kind: &'static str,
    aggregate_id: i64,
    actor: i64,
    recipient: i64,
    object_id: i64,
    dedupe_key: Option<&str>,
) -> Result<(), MailError> {
    sqlx::query(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         VALUES ($1,1,$2,$3,COALESCE($4::text,'tor-social:' || $1 || ':' || gen_random_uuid()::text),
                 jsonb_build_object('actor_user_id',$5,'recipient_user_id',$6,'object_id',$7),
                 ARRAY['user_notifications']::text[])
         ON CONFLICT(dedupe_key) DO NOTHING",
    )
    .bind(event_type)
    .bind(aggregate_kind)
    .bind(aggregate_id)
    .bind(dedupe_key)
    .bind(actor)
    .bind(recipient)
    .bind(object_id)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}
