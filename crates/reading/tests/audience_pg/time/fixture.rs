use sqlx::{postgres::PgPoolOptions, Row};
use tor_reading::services::audience::time::ReadingTimeCounts;

pub use super::super::canonical::fixture::Fixture;

pub async fn fixture(clock: &str) -> Fixture {
    let parsed = chrono::DateTime::parse_from_rfc3339(clock).unwrap();
    let mut f = Fixture::new().await;
    f.sql(
        "CREATE TABLE reader_chapter_starts (LIKE public.reader_chapter_starts INCLUDING ALL);
        CREATE TABLE reading_test_clock(at timestamptz NOT NULL);
        REVOKE ALL ON reader_chapter_starts,reading_test_clock FROM tor_app;
        GRANT SELECT ON reader_chapter_starts,reading_test_clock TO tor_app",
    )
    .await;
    sqlx::query(&format!(
        "INSERT INTO {}.reading_test_clock VALUES(to_timestamp($1::bigint::double precision))",
        f.schema
    ))
    .bind(parsed.timestamp())
    .execute(&f.owner)
    .await
    .unwrap();
    // Test-only DB clock, not a production parameter or replacement SQL query.
    // Explicit pg_catalog position lets the actual port use this stable clock.
    f.sql(&format!(
        "CREATE FUNCTION transaction_timestamp() RETURNS timestamptz LANGUAGE SQL STABLE
         AS 'SELECT at FROM {}.reading_test_clock'",
        f.schema
    ))
    .await;
    f.app.close().await;
    let namespace = f.schema.clone();
    let url = std::env::var("TEST_DATABASE_URL").unwrap(); // validated before base connects/writes
    f.app = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            let schema = namespace.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema},pg_catalog"))
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let row = sqlx::query(
        "SELECT extract(epoch FROM transaction_timestamp())::bigint AS clock,
         current_user AS actor",
    )
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("clock"), parsed.timestamp());
    assert_eq!(row.get::<String, _>("actor"), "tor_app");
    f
}

pub fn at(epoch: i64) -> String {
    format!("to_timestamp({epoch}::double precision)")
}

pub fn counts(starts: i64, chapter_completions: i64) -> Option<ReadingTimeCounts> {
    Some(ReadingTimeCounts {
        starts,
        chapter_completions,
    })
}

pub async fn event(f: &Fixture, chapter: i64, time: &str, finish: bool, mode: &str) {
    assert!(["free", "paid", "invalid", "mismatch", "missing"].contains(&mode));
    let (table, column, kind, free, paid) = if finish {
        (
            "reader_chapter_completions",
            "completed_at",
            "ChapterCompletedV1",
            "C-verified-free-position",
            "C-verified-purchased-position",
        )
    } else {
        (
            "reader_chapter_starts",
            "started_at",
            "ChapterStartedV1",
            "C-verified-free-start",
            "C-verified-purchased-start",
        )
    };
    let purchase = if matches!(mode, "paid" | "mismatch") {
        "123::bigint"
    } else {
        "NULL::bigint"
    };
    let authority = if mode == "invalid" {
        "untrusted"
    } else if matches!(mode, "paid" | "mismatch") {
        paid
    } else {
        free
    };
    let extra = match mode {
        "paid" => "||jsonb_build_object('purchase_id',123)",
        "mismatch" => "||jsonb_build_object('purchase_id',999)",
        _ => "",
    };
    f.sql(&format!(
        "WITH facts AS (
          INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,{column},source_event_id,purchase_id)
          VALUES(1,1,{chapter},{chapter},{time},gen_random_uuid(),{purchase})
          ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING RETURNING *)
         INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         SELECT source_event_id,'{kind}',1,'chapter',chapter_id,source_event_id::text,
           jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,
             'version_id',first_version_id,'authority','{authority}'){extra},ARRAY['fixture-only']
         FROM facts WHERE {}", mode != "missing"
    )).await;
}
