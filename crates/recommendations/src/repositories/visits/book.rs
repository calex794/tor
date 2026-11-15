//! No URL or browser claim can select a discovery channel.
use super::unavailable;
use crate::services::visits::{FlowVisitError, VisitObservation};
use sqlx::{Postgres, Row, Transaction};

pub(super) async fn discovery(
    tx: &mut Transaction<'_, Postgres>,
    observation: &VisitObservation,
) -> Result<i16, FlowVisitError> {
    let Some(book) = observation.book.as_ref() else {
        return Ok(3);
    };
    let expected =
        usize::from(book.rec_assignment.is_some()) + usize::from(book.organic_touch.is_some());
    if expected == 0 {
        return Ok(3);
    }
    let rows = sqlx::query(
        "WITH scope AS (
           SELECT subject_id,flow_enabled_at FROM rec_subjects
           WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid AND enabled
         ), proofs AS (
           SELECT a.clicked_at,0::smallint AS code,0 AS priority FROM rec_assignments a JOIN scope s USING(subject_id)
           WHERE a.id=$4::uuid AND a.book_id=$3 AND a.clicked_at>=s.flow_enabled_at
             AND a.book_opened_at>=a.clicked_at
             AND date_trunc('milliseconds',a.book_opened_at)<=to_timestamp($6::double precision/1000)
             AND a.clicked_at>=to_timestamp($6::double precision/1000)-interval '15 minutes'
           UNION ALL
           SELECT t.clicked_at,(CASE t.source WHEN 'catalog' THEN 1 ELSE 2 END)::smallint,1
           FROM organic_discovery_touches t JOIN scope s USING(subject_id)
           WHERE t.id=$5::uuid AND t.book_id=$3 AND t.clicked_at>=s.flow_enabled_at
             AND t.book_opened_at>=t.clicked_at
             AND date_trunc('milliseconds',t.book_opened_at)<=to_timestamp($6::double precision/1000)
             AND t.clicked_at>=to_timestamp($6::double precision/1000)-interval '15 minutes'
         ) SELECT code FROM proofs ORDER BY clicked_at DESC,priority",
    )
    .bind(&observation.scope.subject_id)
    .bind(&observation.scope.epoch)
    .bind(book.book_id)
    .bind(book.rec_assignment.as_deref())
    .bind(book.organic_touch.as_deref())
    .bind(observation.observed_at_millis)
    .fetch_all(&mut **tx)
    .await
    .map_err(unavailable)?;
    if rows.len() != expected {
        return Err(FlowVisitError::Invalid);
    }
    rows[0].try_get("code").map_err(unavailable)
}
