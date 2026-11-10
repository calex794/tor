use super::fixtures::{count, Fixture, TestResult};
use sqlx::PgPool;

pub async fn foreign_keys_and_fixed_search_path(owner: &PgPool) -> TestResult {
    let expected = [
        ("rec_subjects", vec![
            "flow_book_action_receipts", "flow_book_cohorts", "flow_visit_account_days",
            "flow_visit_continuations", "flow_visit_receipts", "flow_visits",
            "organic_discovery_first_conversions", "organic_discovery_touches",
            "organic_discovery_unattributed_reads", "reader_activity_days",
            "reader_book_return_subjects", "reader_flow_activity_days",
            "reader_flow_book_actions", "reader_flow_read_receipts", "reader_return_subjects",
            "rec_assignments", "rec_feedback_state", "rec_first_conversions", "rec_reader_signals",
        ]),
        ("rec_assignments", vec!["rec_attribution_events", "rec_first_conversions"]),
        ("organic_discovery_touches", vec!["organic_discovery_first_conversions"]),
        ("flow_book_cohorts", vec!["flow_book_return_anchors"]),
    ];
    for (parent, children) in expected {
        let actual: Vec<String> = sqlx::query_scalar(
            "SELECT c.conrelid::regclass::text FROM pg_constraint c
             WHERE c.contype='f' AND c.confrelid=$1::regclass ORDER BY 1",
        ).bind(format!("public.{parent}")).fetch_all(owner).await?;
        assert_eq!(actual, children, "new FK fan-out must be included in the bounded eraser");
    }
    assert_eq!(count(owner,
        "SELECT count(*) FROM pg_constraint WHERE contype='f'
         AND conrelid='rec_analytics_invalidations'::regclass"
    ).await?, 0, "hard deletion must not resurrect a global channel count");
    for function in [
        "rec_reject_retired_collection", "flow_retire_deleted_visits",
        "flow_retire_deleted_book_cohorts", "rec_retire_deleted_assignments",
        "rec_retire_deleted_organic_touches", "rec_retire_deleted_unattributed_reads",
        "rec_retire_deleted_dirty_keys", "flow_allow_only_aggregate_retraction",
        "rec_normalize_retired_outbox_obligations", "rec_reject_retired_delivery",
        "ril_retire_individual_flow_counts", "ril_skip_retired_transition",
        "rec_lock_retired_erasure", "flow_erase_scope", "flow_book_erase_scope",
        "flow_erase_replaced_scope", "flow_book_erase_replaced_scope",
        "reader_flow_erase_read_scope", "reader_flow_erase_read_delivery",
        "reader_return_erase_rotated_consent", "reader_book_return_erase_rotated_consent",
        "reader_flow_capture_book_action", "reader_flow_capture_read", "rec_retire_analytics_batch",
    ] {
        let fixed: bool = sqlx::query_scalar(
            "SELECT COALESCE('search_path=pg_catalog, public, pg_temp'=ANY(proconfig),false)
             FROM pg_proc WHERE pronamespace='public'::regnamespace AND proname=$1",
        ).bind(function).fetch_one(owner).await?;
        assert!(fixed, "unqualified public erasure must not resolve through temp: {function}");
    }
    Ok(())
}

pub async fn temp_shadow_cannot_hide_public_erasure(app: &PgPool, f: &Fixture) -> TestResult {
    let mut tx = app.begin().await?;
    sqlx::raw_sql(
        "SET LOCAL search_path=pg_temp,public;
         CREATE TEMP TABLE flow_visit_daily (LIKE public.flow_visit_daily INCLUDING ALL) ON COMMIT DROP;
         INSERT INTO pg_temp.flow_visit_daily SELECT * FROM public.flow_visit_daily;
         CREATE TEMP TABLE rec_analytics_invalidations
             (LIKE public.rec_analytics_invalidations INCLUDING ALL) ON COMMIT DROP;
         CREATE TEMP TABLE event_deliveries (LIKE public.event_deliveries INCLUDING ALL) ON COMMIT DROP"
    ).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO pg_temp.event_deliveries(consumer,event_id)
         VALUES('flow-read-attribution-v1',$1::uuid),('reading-rankings-v1',$1::uuid)",
    ).bind(&f.event).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM public.flow_visits WHERE subject_id=$1::uuid")
        .bind(&f.subject).execute(&mut *tx).await?;
    let sums: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum(visits)::bigint FROM public.flow_visit_daily),
                (SELECT sum(visits)::bigint FROM pg_temp.flow_visit_daily)",
    ).fetch_one(&mut *tx).await?;
    assert_eq!(sums, (6, 8));
    sqlx::query("DELETE FROM public.rec_assignments WHERE id=$1::uuid")
        .bind(&f.assignment).execute(&mut *tx).await?;
    let markers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.rec_analytics_invalidations WHERE book_id=$1),
                (SELECT count(*) FROM pg_temp.rec_analytics_invalidations)",
    ).bind(f.book).fetch_one(&mut *tx).await?;
    assert_eq!(markers, (1, 0));
    sqlx::query("DELETE FROM public.reader_flow_read_receipts WHERE event_id=$1::uuid")
        .bind(&f.event).execute(&mut *tx).await?;
    let receipts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.event_deliveries
                  WHERE event_id=$1::uuid AND consumer='flow-read-attribution-v1'),
                (SELECT count(*) FROM pg_temp.event_deliveries
                  WHERE event_id=$1::uuid AND consumer='flow-read-attribution-v1')",
    ).bind(&f.event).fetch_one(&mut *tx).await?;
    assert_eq!(receipts, (0, 1));
    sqlx::query("UPDATE public.outbox_events SET required_consumers=required_consumers WHERE id=$1::uuid")
        .bind(&f.event).execute(&mut *tx).await?;
    let delivered: bool = sqlx::query_scalar("SELECT delivered_at IS NOT NULL FROM public.outbox_events WHERE id=$1::uuid")
        .bind(&f.event).fetch_one(&mut *tx).await?;
    assert!(!delivered, "a temp fake ACK cannot close a real ranking obligation");
    tx.rollback().await?;
    Ok(())
}
