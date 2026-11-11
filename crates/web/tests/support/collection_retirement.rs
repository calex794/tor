//! Current-schema no-write proof; legacy installations belong to the upgrade harness.
use super::pg;
use serde_json::Value;

pub(super) fn assert_current_schema(url: &str) {
    assert_eq!(
        pg(
            url,
            "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations
           WHERE version=202611168903 AND success)
         AND EXISTS(SELECT 1 FROM pg_trigger
           WHERE tgrelid='rec_subjects'::regclass
             AND tgname='retired_collection_write_fence' AND tgenabled='O')",
        ),
        "t",
        "HTTP acceptance must exercise the current database barrier"
    );
}

pub(super) fn optional_snapshot(url: &str, actor: i64) -> Value {
    let mut fields = vec![format!(
        "'subject',(SELECT to_jsonb(s) FROM rec_subjects s WHERE user_id={actor})"
    )];
    for table in [
        "rec_assignments",
        "rec_reader_signals",
        "rec_first_conversions",
        "organic_discovery_unattributed_reads",
        "organic_discovery_first_conversions",
        "organic_discovery_touches",
        "flow_visits",
        "flow_visit_receipts",
        "flow_visit_account_days",
        "flow_visit_continuations",
        "flow_book_cohorts",
        "flow_book_action_receipts",
        "flow_book_return_anchors",
        "reader_flow_book_actions",
        "reader_flow_activity_days",
        "reader_flow_read_receipts",
        "reader_activity_days",
        "reader_return_subjects",
        "reader_book_return_subjects",
    ] {
        fields.push(format!(
            "'{table}',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)
              FROM {table} t JOIN rec_subjects s USING(subject_id) WHERE s.user_id={actor})"
        ));
    }
    fields.push(format!(
        "'rec_events',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY to_jsonb(e)::text),'[]'::jsonb)
          FROM rec_attribution_events e JOIN rec_assignments a ON a.id=e.assignment_id
          JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id={actor})"
    ));
    fields.push(format!(
        "'deliveries',(SELECT COALESCE(jsonb_agg(to_jsonb(d) ORDER BY to_jsonb(d)::text),'[]'::jsonb)
          FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
          WHERE e.payload->>'user_id'='{actor}'
            AND d.consumer IN('recommendation-feedback-v1','flow-read-attribution-v1'))"
    ));
    fields.push(format!(
        "'ril_consent',(SELECT to_jsonb(c) FROM ril_behavior_consents c WHERE user_id={actor})"
    ));
    serde_json::from_str(&pg(
        url,
        &format!("SELECT jsonb_build_object({})", fields.join(",")),
    ))
    .unwrap()
}
