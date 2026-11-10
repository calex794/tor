use super::fixtures::{count, text, Fixture, TestResult};
use sqlx::PgPool;

pub fn code(error: sqlx::Error, expected: &str) {
    assert_eq!(
        error
            .as_database_error()
            .expect("PostgreSQL error")
            .code()
            .as_deref(),
        Some(expected),
        "{error}"
    );
}

pub async fn old_writers_and_canonical_noops(app: &PgPool, f: &Fixture) -> TestResult {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
         JOIN pg_namespace n ON n.oid=c.relnamespace
         WHERE t.tgname='retired_collection_write_fence' AND n.nspname='public'
         ORDER BY c.relname",
    )
    .fetch_all(app)
    .await?;
    assert_eq!(tables.len(), 32);
    for table in tables {
        let field = text(
            app,
            &format!(
                "SELECT attname::text FROM pg_attribute WHERE attrelid='public.{table}'::regclass
             AND attnum>0 AND NOT attisdropped AND attidentity='' AND attgenerated=''
             ORDER BY attnum LIMIT 1"
            ),
        )
        .await?;
        let error = sqlx::query(&format!(
            "UPDATE public.{table} SET {field}={field} WHERE false"
        ))
        .execute(app)
        .await
        .unwrap_err();
        code(error, "55000");
    }
    for table in [
        "rec_subjects",
        "rec_assignments",
        "reader_flow_read_receipts",
    ] {
        code(
            sqlx::query(&format!(
                "INSERT INTO {table} SELECT * FROM {table} WHERE false"
            ))
            .execute(app)
            .await
            .unwrap_err(),
            "55000",
        );
    }
    code(sqlx::query(
        "INSERT INTO event_deliveries(consumer,event_id) VALUES('flow-read-attribution-v1',$1::uuid)
         ON CONFLICT DO NOTHING",
    ).bind(&f.event).execute(app).await.unwrap_err(), "55000");
    for view in ["reader_activity_consent", "reader_flow_consent"] {
        let rows = sqlx::query(&format!("SELECT * FROM {view} WHERE user_id=$1 FOR UPDATE"))
            .bind(f.actor)
            .fetch_all(app)
            .await?;
        assert!(
            rows.is_empty(),
            "the old C port stays row-lockable and empty"
        );
    }
    assert_eq!(
        count(
            app,
            &format!(
                "SELECT count(*) FROM reader_flow_verified_read_events WHERE event_id='{}'",
                f.event
            )
        )
        .await?,
        1,
        "necessary historical C evidence is not erased or renamed"
    );
    for view in [
        "reader_flow_verified_book_actions",
        "reader_flow_verified_activity_days",
        "reader_flow_verified_read_receipts",
    ] {
        assert_eq!(
            count(app, &format!("SELECT count(*) FROM {view}")).await?,
            0
        );
    }
    assert_eq!(
        count(
            app,
            "SELECT count(*) FROM reader_return_coverage WHERE ended_at=last_heartbeat_at"
        )
        .await?,
        1
    );
    assert_eq!(
        count(
            app,
            "SELECT count(*) FROM reader_book_return_coverage WHERE ended_at=last_heartbeat_at"
        )
        .await?,
        1
    );
    // Actual old producer GUCs are enabled. Canonical save/follow and read rows
    // still commit; no trigger may recreate a derived scope or proof.
    let mut tx = app.begin().await?;
    sqlx::raw_sql(
        "SET LOCAL tor.flow_capture_enabled='1'; SET LOCAL tor.flow_read_capture_enabled='1'",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO reader_library_items(user_id,book_id) VALUES($1,$2)")
        .bind(f.actor)
        .bind(f.untouched_book)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(f.actor)
        .bind(f.untouched_book)
        .execute(&mut *tx)
        .await?;
    let completion: String = sqlx::query_scalar(
        "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterCompletedV1',1,'chapter',$1,'ci:retirement:after-cutover',
            jsonb_build_object('user_id',$2::bigint::text,'book_id',$3::bigint::text,
                'chapter_id',$1::bigint::text,'version_id',$4::bigint::text,
                'authority','C-verified-free-position'),
            ARRAY['recommendation-feedback-v1'])
         RETURNING id::text",
    )
    .bind(f.chapter)
    .bind(f.actor)
    .bind(f.book)
    .bind(f.version)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,source_event_id)
         VALUES($1,$2,$3,$4,$5::uuid)",
    ).bind(f.actor).bind(f.book).bind(f.chapter).bind(f.version).bind(&completion)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    assert_eq!(
        count(
            app,
            &format!(
                "SELECT count(*) FROM reader_flow_book_actions WHERE book_id={}",
                f.untouched_book
            )
        )
        .await?,
        0
    );
    assert_eq!(
        count(
            app,
            &format!(
                "SELECT count(*) FROM reader_flow_read_receipts WHERE event_id='{completion}'"
            )
        )
        .await?,
        0
    );
    assert_eq!(
        text(
            app,
            &format!(
        "SELECT concat_ws('|',cardinality(required_consumers),(delivered_at IS NOT NULL)::int,
             (lease_until IS NULL)::int) FROM outbox_events WHERE id='{completion}'"
    )
        )
        .await?,
        "0|1|1"
    );
    assert_eq!(
        count(
            app,
            &format!("SELECT count(*) FROM event_deliveries WHERE event_id='{completion}'")
        )
        .await?,
        0,
        "empty obligations do not fabricate individual delivery receipts"
    );
    Ok(())
}

pub async fn scope_cascade_and_integrity(app: &PgPool, f: &Fixture) -> TestResult {
    let before = text(app,
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY cohort_day,landing_surface)::text FROM flow_visit_daily d"
    ).await?;
    let mut tx = app.begin().await?;
    for _ in 0..2 {
        sqlx::query("SELECT flow_erase_scope($1::uuid,$2::uuid)")
            .bind(&f.subject)
            .bind(&f.epoch)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT flow_book_erase_scope($1::uuid,$2::uuid)")
            .bind(&f.subject)
            .bind(&f.epoch)
            .execute(&mut *tx)
            .await?;
    }
    let visits: i64 = sqlx::query_scalar("SELECT sum(visits)::bigint FROM flow_visit_daily")
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(visits, 6);
    let books: i64 = sqlx::query_scalar("SELECT cohorts FROM flow_book_daily WHERE book_id=$1")
        .bind(f.book)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(
        books, 1,
        "the explicit scope call and retained hooks retract exactly once"
    );
    code(sqlx::query(
        "UPDATE rec_subjects SET enabled=false,flow_epoch=NULL,flow_enabled_at=NULL WHERE user_id=$1",
    ).bind(f.actor).execute(&mut *tx).await.unwrap_err(), "55000");
    tx.rollback().await?;
    assert_eq!(text(app,
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY cohort_day,landing_surface)::text FROM flow_visit_daily d"
    ).await?, before, "failed old consent transaction rolls back its preceding erasure");

    let mut tx = app.begin().await?;
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(f.cascade_book)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM flow_book_daily WHERE book_id=$1")
            .bind(f.cascade_book)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        remaining, 0,
        "whole-book canonical deletion also removes its historical daily"
    );
    tx.rollback().await?;

    let mut tx = app.begin().await?;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(f.cascade_actor)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let remaining: i64 = sqlx::query_scalar("SELECT cohorts FROM flow_book_daily WHERE book_id=$1")
        .bind(f.cascade_book)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(
        remaining, 1,
        "account cascade retracts only retained linked contributions"
    );
    tx.rollback().await?;

    for book_case in [false, true] {
        let mut tx = app.begin().await?;
        if book_case {
            sqlx::query("DELETE FROM flow_book_daily WHERE book_id=$1")
                .bind(f.book)
                .execute(&mut *tx)
                .await?;
            code(
                sqlx::query("DELETE FROM flow_book_cohorts WHERE subject_id=$1::uuid")
                    .bind(&f.subject)
                    .execute(&mut *tx)
                    .await
                    .unwrap_err(),
                "23514",
            );
        } else {
            sqlx::query("DELETE FROM flow_visit_daily WHERE landing_surface=1")
                .execute(&mut *tx)
                .await?;
            code(
                sqlx::query("DELETE FROM flow_visits WHERE subject_id=$1::uuid")
                    .bind(&f.subject)
                    .execute(&mut *tx)
                    .await
                    .unwrap_err(),
                "23514",
            );
        }
        tx.rollback().await?;
    }
    code(
        sqlx::query("UPDATE flow_visit_daily SET visits=visits+1")
            .execute(app)
            .await
            .unwrap_err(),
        "55000",
    );
    code(
        sqlx::query("INSERT INTO flow_book_daily SELECT * FROM flow_book_daily WHERE false")
            .execute(app)
            .await
            .unwrap_err(),
        "55000",
    );
    Ok(())
}
