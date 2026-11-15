use super::{
    fixtures::{count, run, text, Fixture, TestResult},
    proofs::code,
};
use sqlx::{PgPool, Postgres, Transaction};

const DELETED: &[&str] = &[
    "rec_dirty_buckets",
    "organic_discovery_dirty",
    "rec_attribution_events",
    "rec_first_conversions",
    "organic_discovery_first_conversions",
    "flow_book_return_anchors",
    "event_deliveries",
    "rec_assignments",
    "organic_discovery_touches",
    "organic_discovery_unattributed_reads",
    "rec_reader_signals",
    "flow_book_cohorts",
    "flow_visits",
    "flow_visit_receipts",
    "flow_visit_account_days",
    "flow_visit_continuations",
    "flow_book_action_receipts",
    "reader_flow_book_actions",
    "reader_flow_activity_days",
    "reader_flow_read_receipts",
    "reader_activity_days",
    "reader_return_subjects",
    "reader_book_return_subjects",
    "ril_behavior_consents",
    "ril_flush_transitions",
    "rec_subjects",
    "rec_quality_daily",
    "organic_discovery_quality_daily",
    "flow_visit_daily",
    "flow_book_daily",
    "reader_return_daily",
    "reader_book_return_daily",
    "reader_return_coverage",
    "reader_book_return_coverage",
    "rec_analytics_invalidations",
];

async fn install_numeric_audit(owner: &PgPool) -> TestResult {
    // Test-only numeric counts, no identifying payload. Observe actual trigger
    // and FK fan-out rather than trusting the SQL port's own return value.
    run(
        owner,
        "CREATE TABLE ci_retirement_work(
            transaction_id bigint NOT NULL DEFAULT txid_current(),
            relation_name text NOT NULL,operation text NOT NULL,rows bigint NOT NULL);
         CREATE FUNCTION ci_retirement_count() RETURNS trigger LANGUAGE plpgsql
         SET search_path=pg_catalog,public,pg_temp AS $$
         DECLARE n bigint;
         BEGIN
           SELECT count(*) INTO n FROM changed_rows;
           IF n>0 THEN
             INSERT INTO ci_retirement_work(relation_name,operation,rows)
             VALUES(TG_TABLE_NAME,TG_OP,n);
           END IF;
           RETURN NULL;
         END $$;
         GRANT SELECT,INSERT,DELETE ON ci_retirement_work TO tor_app",
    )
    .await?;
    for table in DELETED {
        run(
            owner,
            &format!(
                "CREATE TRIGGER ci_retirement_count_delete AFTER DELETE ON {table}
             REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT
             EXECUTE FUNCTION ci_retirement_count()"
            ),
        )
        .await?;
    }
    for table in [
        "outbox_events",
        "ril_flush_flow",
        "flow_visit_daily",
        "flow_book_daily",
    ] {
        run(
            owner,
            &format!(
                "CREATE TRIGGER ci_retirement_count_update AFTER UPDATE ON {table}
             REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT
             EXECUTE FUNCTION ci_retirement_count()"
            ),
        )
        .await?;
    }
    run(
        owner,
        "CREATE TRIGGER ci_retirement_count_insert AFTER INSERT ON rec_analytics_invalidations
         REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT
         EXECUTE FUNCTION ci_retirement_count()",
    )
    .await
}

async fn assert_budget(tx: &mut Transaction<'_, Postgres>, limit: i32, erased: i64) -> TestResult {
    let (deletes, changed, markers, retractions): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(rows) FILTER(WHERE operation='DELETE'),0)::bigint,
                COALESCE(sum(rows) FILTER(WHERE operation='UPDATE'
                    AND relation_name IN ('outbox_events','ril_flush_flow')),0)::bigint,
                COALESCE(sum(rows) FILTER(WHERE operation='INSERT'),0)::bigint,
                COALESCE(sum(rows) FILTER(WHERE operation='UPDATE'
                    AND relation_name IN ('flow_visit_daily','flow_book_daily')),0)::bigint
         FROM ci_retirement_work WHERE transaction_id=txid_current()",
    )
    .fetch_one(&mut **tx)
    .await?;
    assert_eq!(
        deletes, erased,
        "including all real FK and trigger DELETE fan-out"
    );
    assert!(
        deletes + changed <= i64::from(limit),
        "one shared work-row budget"
    );
    assert!(markers <= i64::from(limit) * 3);
    assert!(
        retractions <= deletes,
        "at most one daily update per deleted finalized input"
    );
    Ok(())
}

async fn batch(app: &PgPool, limit: i32) -> TestResult<(i64, bool)> {
    let mut tx = app.begin().await?;
    let result: (i64, bool) = sqlx::query_as("SELECT * FROM rec_retire_analytics_batch($1)")
        .bind(limit)
        .fetch_one(&mut *tx)
        .await?;
    assert!((0..=i64::from(limit)).contains(&result.0));
    assert_budget(&mut tx, limit, result.0).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn bounded_rollback_workers_and_retention(
    owner: &PgPool,
    app: &PgPool,
    f: &Fixture,
) -> TestResult {
    install_numeric_audit(owner).await?;
    for limit in [Some(0), Some(-1), Some(1001), None] {
        code(
            sqlx::query("SELECT * FROM rec_retire_analytics_batch($1)")
                .bind(limit)
                .execute(app)
                .await
                .unwrap_err(),
            "22023",
        );
    }
    let before = count(app, "SELECT count(*) FROM rec_attribution_events").await?;
    let mut rollback = app.begin().await?;
    let result: (i64, bool) = sqlx::query_as("SELECT * FROM rec_retire_analytics_batch(7)")
        .fetch_one(&mut *rollback)
        .await?;
    assert_budget(&mut rollback, 7, result.0).await?;
    rollback.rollback().await?;
    assert_eq!(
        count(app, "SELECT count(*) FROM rec_attribution_events").await?,
        before
    );
    assert_eq!(
        count(app, "SELECT count(*) FROM ci_retirement_work").await?,
        0
    );

    let untouched = text(
        app,
        &format!(
            "SELECT jsonb_agg(to_jsonb(d) ORDER BY bucket_date)::text FROM rec_quality_daily d
         WHERE book_id={}",
            f.untouched_book
        ),
    )
    .await?;
    // Both workers are real independent transactions. An advisory-busy result
    // is allowed, but cannot duplicate a marker, retraction or deletion.
    let (left, right) = tokio::join!(batch(app, 2), batch(app, 2));
    left?;
    right?;

    // A marker cannot expire while an old matching daily row remains locked.
    // Markers are anonymous absence evidence, seeded only to exercise TTL.
    run(
        owner,
        &format!(
            "INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
         SELECT 'recommendations',(now() AT TIME ZONE 'UTC')::date-age,{}
         FROM unnest(ARRAY[180,181,182]) age",
            f.boundary_book
        ),
    )
    .await?;
    let mut held = Some(app.begin().await?);
    sqlx::query(
        "SELECT 1 FROM rec_quality_daily WHERE book_id=$1
         AND bucket_date=(now() AT TIME ZONE 'UTC')::date-181 FOR UPDATE",
    )
    .bind(f.boundary_book)
    .fetch_all(&mut **held.as_mut().unwrap())
    .await?;
    let mut completed = false;
    let mut held_marker_observed = false;
    for step in 0..200 {
        let (erased, pending) = batch(app, if step % 2 == 0 { 1 } else { 7 }).await?;
        if erased == 0 && pending && held.is_some() {
            // Finish any remaining normalization while the same row is held,
            // then prove a full pass cannot expire its absence marker.
            assert!(batch(app, 1000).await?.1);
            assert_eq!(batch(app, 1000).await?, (0, true));
            assert_eq!(
                count(
                    app,
                    &format!(
                        "SELECT count(*) FROM rec_analytics_invalidations WHERE book_id={}
                 AND bucket_date=(now() AT TIME ZONE 'UTC')::date-181",
                        f.boundary_book
                    )
                )
                .await?,
                1,
                "no gap where a masked still-present count can reappear"
            );
            held_marker_observed = true;
            held.take().unwrap().rollback().await?;
        }
        if !pending {
            completed = true;
            break;
        }
    }
    assert!(held_marker_observed && held.is_none());
    assert!(
        completed,
        "bounded retries must eventually drain unblocked work"
    );
    assert_eq!(batch(app, 1000).await?, (0, false));
    assert_eq!(count(app, "SELECT count(*) FROM rec_subjects").await?, 0);
    for relation in [
        "rec_assignments",
        "rec_attribution_events",
        "rec_first_conversions",
        "rec_reader_signals",
        "organic_discovery_touches",
        "organic_discovery_first_conversions",
        "organic_discovery_unattributed_reads",
        "flow_visits",
        "flow_book_cohorts",
        "reader_flow_read_receipts",
        "reader_activity_days",
        "reader_return_subjects",
        "reader_book_return_subjects",
        "ril_behavior_consents",
    ] {
        assert_eq!(
            count(app, &format!("SELECT count(*) FROM {relation}")).await?,
            0,
            "{relation}"
        );
    }
    let sources: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT source FROM rec_analytics_invalidations WHERE book_id=$1 ORDER BY source",
    )
    .bind(f.book)
    .fetch_all(app)
    .await?;
    assert_eq!(sources, ["catalog", "recommendations", "unattributed"]);
    let sources: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT source FROM rec_analytics_invalidations WHERE book_id=$1 ORDER BY source",
    )
    .bind(f.dirty_book)
    .fetch_all(app)
    .await?;
    assert_eq!(
        sources,
        ["catalog", "recommendations", "search", "unattributed"]
    );
    assert_eq!(
        count(
            app,
            &format!(
                "SELECT count(*) FROM rec_analytics_invalidations WHERE book_id={}",
                f.untouched_book
            )
        )
        .await?,
        0
    );
    assert_eq!(
        text(
            app,
            &format!(
                "SELECT jsonb_agg(to_jsonb(d) ORDER BY bucket_date)::text FROM rec_quality_daily d
         WHERE book_id={}",
                f.untouched_book
            )
        )
        .await?,
        untouched,
        "untouched history must not be destroyed"
    );
    assert_eq!(
        count(
            app,
            &format!(
        "SELECT starts FROM organic_discovery_quality_daily WHERE book_id={} AND source='search'",
        f.book
    )
        )
        .await?,
        3,
        "another channel of the same book is retained"
    );
    assert_eq!(text(app, &format!(
        "SELECT string_agg(((now() AT TIME ZONE 'UTC')::date-bucket_date)::text,',' ORDER BY bucket_date)
         FROM rec_analytics_invalidations WHERE book_id={}", f.boundary_book
    )).await?, "180", "retain the exact existing 180-day edge");
    assert_eq!(
        count(app, "SELECT sum(visits)::bigint FROM flow_visit_daily").await?,
        4
    );
    assert_eq!(
        count(app, "SELECT sum(cohorts)::bigint FROM flow_book_daily").await?,
        2
    );
    assert_eq!(
        text(
            app,
            &format!(
                "SELECT concat_ws('|',array_to_string(required_consumers,','),
             (lease_until IS NOT NULL)::int,(delivered_at IS NULL)::int)
         FROM outbox_events WHERE id='{}'",
                f.event
            )
        )
        .await?,
        "reading-rankings-v1|1|1"
    );
    assert_eq!(
        count(
            app,
            &format!(
        "SELECT count(*) FROM reader_chapter_completions WHERE user_id={} AND chapter_id={}",
        f.actor, f.chapter
    )
        )
        .await?,
        1
    );
    assert_eq!(
        count(
            app,
            "SELECT count(*) FROM event_deliveries
         WHERE consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')"
        )
        .await?,
        0
    );
    Ok(())
}
