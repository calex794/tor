//! FLOW: independent bounded book observations through real C progress writers.
//! Temporal fixtures are evidence only; production never backfills reading.
#[path = "audience_book_returns_pg/fixtures.rs"]
mod fixtures;

use fixtures::*;
use sqlx::PgPool;
use tor_reading::{
    repositories::{returns::PgReadingReturns, PgReadingProgress, PgReadingSync},
    services::{progress::ProgressError, returns::ReadingReturnsError, sync::SyncError},
};

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn flow_book_returns_bounded_episodes_writers_privacy_and_exact_utc() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    let pool = PgPool::connect(&url).await.unwrap();
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COALESCE(max(id),0) FROM reader_book_return_coverage),
                (SELECT COALESCE(max(id),0) FROM reader_return_coverage)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let report = PgReadingReturns::new(pool.clone());
    let sync = PgReadingSync::new(pool.clone()).with_book_return_collection(true);
    let mut actors = Vec::new();
    let mut books = Vec::new();
    independent_start(&pool, &report, &sync, &mut actors, &mut books).await;
    writer_cases(&pool, &report, &sync, &mut actors, &mut books).await;
    paid_and_archived_cases(&pool, &sync, &mut actors, &mut books).await;
    temporal_cases(&pool, &report, &sync, &mut actors, &mut books).await;
    renewal_and_expiry(&pool, &report, &sync, &mut actors, &mut books).await;
    multiple_batches(&pool, &report, &mut books).await;
    failure_and_consent_races(&pool, &report, &sync, &mut actors, &mut books).await;
    for actor in actors {
        consent(&pool, actor, false).await;
    }
    report.tick_book_collection(false).await.unwrap();
    report.tick_collection(false).await.unwrap();
    sqlx::query("DELETE FROM reader_book_return_daily WHERE book_id=ANY($1)")
        .bind(&books)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM reader_book_return_coverage WHERE id>$1")
        .bind(before.0)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM reader_return_coverage WHERE id>$1 AND NOT EXISTS(SELECT 1 FROM reader_return_subjects s WHERE s.coverage_epoch_id=reader_return_coverage.id)")
        .bind(before.1).execute(&pool).await.unwrap();
    pool.close().await;
}

async fn independent_start(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    report.start_collection(true).await.unwrap();
    report.tick_book_collection(false).await.unwrap();
    let a = actor(pool, "independent").await;
    actors.push(a);
    consent(pool, a, true).await;
    let one = book(pool, "independent", false, books).await;
    sync.save_free(a, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, a).await,
        0,
        "global coverage is not book coverage"
    );
    let empty = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert!(!empty.collection_healthy);
    assert_eq!(empty.observation_started_at_utc, None);
    assert!(!empty.current.coverage_complete);
    report.start_book_collection(true).await.unwrap();
    PgReadingSync::new(pool.clone())
        .save_free(a, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, a).await,
        0,
        "new builder defaults to disabled"
    );
    sync.save_free(a, one.chapter, one.version, place(3), 2)
        .await
        .unwrap();
    assert_eq!(episodes(pool, a).await, 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM reader_activity_days d JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(a).fetch_one(pool).await.unwrap(), 0, "book flag does not extend global raw history");
    let current = active(pool).await;
    report.start_book_collection(true).await.unwrap();
    assert_ne!(
        active(pool).await.0,
        current.0,
        "restart creates a real boundary"
    );
    let restarted = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted.current.enrolled, 1);
    assert!(restarted.collection_healthy);
    assert!(!restarted.current.coverage_complete);
    assert_eq!(
        report.book_returns_report(0, 90).await,
        Err(ReadingReturnsError::Invalid)
    );
    assert_eq!(
        report.book_returns_report(one.id, 2).await,
        Err(ReadingReturnsError::Invalid)
    );
    assert!(report
        .book_returns_report(i64::MAX, 90)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(one.id)
        .execute(pool)
        .await
        .unwrap();
    assert!(report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE books SET admin_hold=false,title='Renamed book' WHERE id=$1")
        .bind(one.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .book_title,
        "Renamed book"
    );
}

async fn writer_cases(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let one = book(pool, "free", false, books).await;
    let two = book(pool, "second-book", false, books).await;
    let a = actor(pool, "free").await;
    actors.push(a);
    sync.save_free(a, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(episodes(pool, a).await, 0, "default consent is off");
    consent(pool, a, true).await;
    // A completed global marker must not suppress this independently enabled path.
    sqlx::query("INSERT INTO reader_return_subjects(subject_id,enrolled) SELECT subject_id,true FROM rec_subjects WHERE user_id=$1")
        .bind(a).execute(pool).await.unwrap();
    let both = sync.clone().with_activity_collection(true);
    both.save_free(a, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(episodes(pool, a).await, 1);
    assert_eq!(
        sync.save_free(a, one.chapter, one.version, place(2), 1)
            .await,
        Err(SyncError::Conflict)
    );
    assert_eq!(
        sync.save_free(a, one.chapter, one.version, place(99), 2)
            .await,
        Err(SyncError::Invalid)
    );
    sync.save_free(a, two.chapter, two.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, a).await,
        2,
        "different books never share observations"
    );
    sqlx::query("UPDATE reader_book_return_subjects SET cohort_day=(now() AT TIME ZONE 'UTC')::date-7 WHERE book_id=$1")
        .bind(one.id).execute(pool).await.unwrap();
    sync.save_free(a, two.chapter, two.version, place(2), 1)
        .await
        .unwrap();
    let flag = || {
        sqlx::query_scalar::<_, bool>(
            "SELECT returned_d7 FROM reader_book_return_subjects WHERE book_id=$1",
        )
        .bind(one.id)
    };
    assert!(
        !flag().fetch_one(pool).await.unwrap(),
        "another book cannot count as a return"
    );
    let chapter: i64 = sqlx::query_scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) VALUES($1,2,'Next chapter','public','free') RETURNING id")
        .bind(one.id).fetch_one(pool).await.unwrap();
    let version: i64 = sqlx::query_scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES($1,1,'abcdefghij') RETURNING id")
        .bind(chapter).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    sync.save_free(a, chapter, version, place(1), 2)
        .await
        .unwrap();
    sync.save_free(a, chapter, version, place(2), 3)
        .await
        .unwrap();
    assert!(flag().fetch_one(pool).await.unwrap());
    assert_eq!(
        episodes(pool, a).await,
        2,
        "chapters in a book do not multiply an episode"
    );
    let first = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first.current,
        PgReadingReturns::new(pool.clone())
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .current
    );

    let b = actor(pool, "manual").await;
    actors.push(b);
    consent(pool, b, true).await;
    sync.save_free(b, one.chapter, one.version, place(0), 0)
        .await
        .unwrap();
    assert_eq!(episodes(pool, b).await, 0, "initial anchor is not reading");
    let manual = PgReadingProgress::new(pool.clone()).with_book_return_collection(true);
    assert_eq!(
        manual
            .save_position(b, one.chapter, one.version, place(99))
            .await,
        Err(ProgressError::Invalid)
    );
    manual
        .save_position(b, one.chapter, one.version, place(2))
        .await
        .unwrap();
    assert_eq!(episodes(pool, b).await, 1);
    let old: String =
        sqlx::query_scalar("SELECT subject_id::text FROM rec_subjects WHERE user_id=$1")
            .bind(b)
            .fetch_one(pool)
            .await
            .unwrap();
    consent(pool, b, false).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reader_book_return_subjects WHERE subject_id=$1::uuid"
        )
        .bind(old)
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    consent(pool, b, true).await;
    manual
        .save_position(b, one.chapter, one.version, place(2))
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, b).await,
        0,
        "manual no-op cannot backfill after new consent"
    );
    manual
        .save_position(b, one.chapter, one.version, place(3))
        .await
        .unwrap();
    assert_eq!(episodes(pool, b).await, 1);

    let c = actor(pool, "devices").await;
    actors.push(c);
    consent(pool, c, true).await;
    let (left, right) = tokio::join!(
        sync.save_free(c, one.chapter, one.version, place(1), 0),
        sync.save_free(c, one.chapter, one.version, place(2), 0)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(episodes(pool, c).await, 1);
    let deleted = actor(pool, "deleted").await;
    consent(pool, deleted, true).await;
    cohort(pool, deleted, one.id, 1, [false; 2]).await;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(deleted)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, deleted).await,
        0,
        "account deletion cascades personal observations"
    );
}

async fn temporal_cases(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let coverage = active(pool).await.0;
    sqlx::query("UPDATE reader_book_return_coverage SET started_at=((now() AT TIME ZONE 'UTC')::date-8)::timestamp AT TIME ZONE 'UTC',last_heartbeat_at=now() WHERE id=$1")
        .bind(coverage).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at,ended_at) VALUES(((now() AT TIME ZONE 'UTC')::date-200)::timestamp AT TIME ZONE 'UTC',(((now() AT TIME ZONE 'UTC')::date-10)::timestamp+interval '12 hours') AT TIME ZONE 'UTC',(((now() AT TIME ZONE 'UTC')::date-10)::timestamp+interval '12 hours') AT TIME ZONE 'UTC')")
        .execute(pool).await.unwrap();
    let one = book(pool, "cohorts", false, books).await;
    let cases = [
        (8, [true, false]),
        (9, [false; 2]),
        (17, [true; 2]),
        (7, [false; 2]),
        (0, [false; 2]),
        (31, [true; 2]),
        (40, [true; 2]),
        (100, [true; 2]),
    ];
    let mut today_seven = 0;
    for (index, (age, flags)) in cases.into_iter().enumerate() {
        let a = actor(pool, &format!("cohort-{index}")).await;
        actors.push(a);
        consent(pool, a, true).await;
        cohort(pool, a, one.id, age, flags).await;
        if age == 7 {
            today_seven = a;
        }
    }
    let thirty = actor(pool, "today-thirty").await;
    actors.push(thirty);
    consent(pool, thirty, true).await;
    cohort(pool, thirty, one.id, 30, [false; 2]).await;
    sync.save_free(today_seven, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    sync.save_free(thirty, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    let flags: Vec<(i64, bool, bool)> = sqlx::query_as("SELECT s.user_id,r.returned_d7,r.returned_d30 FROM reader_book_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=ANY($1) ORDER BY s.user_id")
        .bind(vec![today_seven, thirty]).fetch_all(pool).await.unwrap();
    assert_eq!(
        flags,
        vec![(today_seven, true, false), (thirty, false, true)]
    );
    // Full-period sums must not inherit a detailed list's common LIMIT 100.
    for index in 0..101 {
        let a = actor(pool, &format!("wide-{index}")).await;
        actors.push(a);
        consent(pool, a, true).await;
        cohort(pool, a, one.id, 8, [true, false]).await;
    }
    let result = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.current.enrolled, 109);
    assert_eq!(result.previous.enrolled, 1);
    let d7 = result.current.metrics[0];
    let d30 = result.current.metrics[1];
    assert_eq!(
        (d7.eligible, d7.returned, d7.pending, d7.unknown),
        (106, 104, 2, 1)
    );
    assert_eq!(
        (d30.eligible, d30.returned, d30.pending, d30.unknown),
        (1, 1, 107, 1)
    );
    assert_eq!(result.previous.metrics[0].returned, 1);
    assert_eq!(result.previous.metrics[1].returned, 1);
    for days in [1, 7, 30, 90] {
        let selected = report
            .book_returns_report(one.id, days)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(selected.current.from_utc, selected.previous.until_utc);
        for period in [&selected.current, &selected.previous] {
            let from = chrono::DateTime::parse_from_rfc3339(&period.from_utc).unwrap();
            let until = chrono::DateTime::parse_from_rfc3339(&period.until_utc).unwrap();
            assert_eq!((until - from).num_seconds(), i64::from(days) * 86_400);
            for metric in period.metrics {
                assert_eq!(
                    metric.eligible + metric.pending + metric.unknown,
                    period.enrolled
                );
                assert!(metric.returned <= metric.eligible);
            }
        }
    }
    let zero = book(pool, "zero", false, books).await;
    let z = actor(pool, "zero").await;
    actors.push(z);
    consent(pool, z, true).await;
    cohort(pool, z, zero.id, 8, [false; 2]).await;
    sync.save_free(z, zero.chapter, zero.version, place(1), 0)
        .await
        .unwrap();
    let late: (bool, bool) = sqlx::query_as(
        "SELECT returned_d7,returned_d30 FROM reader_book_return_subjects WHERE book_id=$1",
    )
    .bind(zero.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        late,
        (false, false),
        "D8 reading cannot manufacture an exact D7 return"
    );
    let zero = report
        .book_returns_report(zero.id, 90)
        .await
        .unwrap()
        .unwrap()
        .current
        .metrics[0];
    assert_eq!(
        (zero.eligible, zero.returned, zero.pending, zero.unknown),
        (1, 0, 0, 0)
    );
    report.tick_book_collection(true).await.unwrap();
    let summarized = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.current, summarized.current,
        "anonymization preserves counts"
    );
    assert_eq!(result.previous, summarized.previous);
    report.start_book_collection(true).await.unwrap();
    let restarted = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restarted.current.metrics[0].unknown,
        d7.unknown + 1,
        "today's D7 gap is unknown"
    );
    assert_eq!(
        restarted.current.metrics[1].unknown,
        d30.unknown + 1,
        "today's D30 gap is unknown"
    );
    assert_eq!(restarted.current.metrics[0].returned, d7.returned);
}

async fn renewal_and_expiry(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let one = book(pool, "renewal", false, books).await;
    let a = actor(pool, "renewal").await;
    actors.push(a);
    consent(pool, a, true).await;
    cohort(pool, a, one.id, 31, [true; 2]).await;
    sync.save_free(a, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, a).await,
        2,
        "successor starts even before expired cleanup"
    );
    let before = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    report.tick_book_collection(true).await.unwrap();
    assert_eq!(
        episodes(pool, a).await,
        1,
        "finalizer deletes only its selected triple"
    );
    assert!(sqlx::query_scalar::<_, bool>("SELECT cohort_day=(now() AT TIME ZONE 'UTC')::date FROM reader_book_return_subjects WHERE book_id=$1")
        .bind(one.id).fetch_one(pool).await.unwrap());
    let after = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.current, after.current);
    report.tick_book_collection(true).await.unwrap();
    assert_eq!(
        after.current,
        report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .current
    );
    consent(pool, a, false).await;
    assert_eq!(episodes(pool, a).await, 0);
    assert_eq!(
        report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .current
            .enrolled,
        1,
        "anonymous previous observation has no revocable personal link"
    );
    sqlx::query("INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown) SELECT $1,(now() AT TIME ZONE 'UTC')::date-age,day,1,1,0,0 FROM (VALUES(179),(180)) ages(age) CROSS JOIN (VALUES(7),(30)) horizons(day)")
        .bind(one.id).execute(pool).await.unwrap();
    report.tick_book_collection(true).await.unwrap();
    let ages: Vec<i32> = sqlx::query_scalar("SELECT DISTINCT (now() AT TIME ZONE 'UTC')::date-cohort_day FROM reader_book_return_daily WHERE book_id=$1 ORDER BY 1")
        .bind(one.id).fetch_all(pool).await.unwrap();
    assert_eq!(
        ages,
        vec![31, 179],
        "anonymous rows use a strict 180-cohort-day lifetime"
    );
    let active_id = active(pool).await.0;
    sqlx::query("UPDATE reader_book_return_coverage SET started_at=now()-interval '2 minutes',last_heartbeat_at=now()-interval '61 seconds' WHERE id=$1")
        .bind(active_id).execute(pool).await.unwrap();
    let stale = actor(pool, "stale").await;
    actors.push(stale);
    consent(pool, stale, true).await;
    sync.save_free(stale, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, stale).await,
        0,
        "stale supervision cannot collect"
    );
    assert!(
        !report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .collection_healthy
    );
    report.tick_book_collection(true).await.unwrap();
    assert_ne!(active(pool).await.0, active_id);
    sync.save_free(stale, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(episodes(pool, stale).await, 1);
}

async fn multiple_batches(pool: &PgPool, report: &PgReadingReturns, books: &mut Vec<i64>) {
    let one = book(pool, "batch", false, books).await;
    let historical: i64 = sqlx::query_scalar("INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at,ended_at) VALUES(((now() AT TIME ZONE 'UTC')::date-33)::timestamp AT TIME ZONE 'UTC',((now() AT TIME ZONE 'UTC')::date-1)::timestamp AT TIME ZONE 'UTC',((now() AT TIME ZONE 'UTC')::date-1)::timestamp AT TIME ZONE 'UTC') RETURNING id")
        .fetch_one(pool).await.unwrap();
    let actors: Vec<i64> = sqlx::query_scalar(
        "WITH added AS (
           INSERT INTO users(email_normalized,display_name,password_hash)
           SELECT concat('book-returns-batch-',$1::bigint::text,'-',n,'@example.invalid'),'Book return batch','$argon2id$fixture'
           FROM generate_series(1,601) n RETURNING id
         ), consented AS (
           INSERT INTO rec_subjects(user_id,enabled,enabled_at)
           SELECT id,true,now() FROM added RETURNING user_id,subject_id
         ), cohorts AS (
           INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day,returned_d7,returned_d30)
           SELECT subject_id,$1::bigint,(now() AT TIME ZONE 'UTC')::date-32,false,true FROM consented
         ) SELECT user_id FROM consented ORDER BY user_id",
    ).bind(one.id).fetch_all(pool).await.unwrap();
    let previous = active(pool).await;
    let started = std::time::Instant::now();
    report.tick_book_collection(true).await.unwrap();
    eprintln!(
        "book returns: 601 observations finalized in {:?}",
        started.elapsed()
    );
    assert_eq!(
        active(pool).await,
        previous,
        "multiple bounded batches preserve healthy coverage"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reader_book_return_subjects WHERE book_id=$1"
        )
        .bind(one.id)
        .fetch_one(pool)
        .await
        .unwrap(),
        0,
        "no permanent per-book marker survives"
    );
    let rows: Vec<(i32,i64,i64,i64,i64)> = sqlx::query_as("SELECT day::int,enrolled,eligible,returned,unknown FROM reader_book_return_daily WHERE book_id=$1 ORDER BY day")
        .bind(one.id).fetch_all(pool).await.unwrap();
    assert_eq!(rows, vec![(7, 601, 601, 0, 0), (30, 601, 601, 601, 0)]);
    let first = report
        .book_returns_report(one.id, 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.current.enrolled, 601);
    report.tick_book_collection(true).await.unwrap();
    assert_eq!(
        first.current,
        report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .current
    );
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(&actors)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM reader_book_return_coverage WHERE id=$1")
        .bind(historical)
        .execute(pool)
        .await
        .unwrap();
}

async fn failure_and_consent_races(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let owner_url = std::env::var("MIGRATOR_URL").expect("disposable CI schema owner");
    assert!(owner_url.ends_with("/tor_ci") && owner_url.contains("@127.0.0.1:"));
    let owner = PgPool::connect(&owner_url).await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (String, String)>("SELECT current_database(),current_user")
            .fetch_one(&owner)
            .await
            .unwrap(),
        ("tor_ci".into(), "tor_migrator".into())
    );
    let one = book(pool, "fault", false, books).await;
    let a = actor(pool, "fault").await;
    actors.push(a);
    consent(pool, a, true).await;
    report.tick_collection(true).await.unwrap();
    sqlx::raw_sql(&format!("CREATE FUNCTION ci_book_return_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.book_id={} THEN RAISE EXCEPTION 'fixture book derivative failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER ci_book_return_fail BEFORE INSERT ON reader_book_return_subjects FOR EACH ROW EXECUTE FUNCTION ci_book_return_fail();", one.id))
        .execute(&owner).await.unwrap();
    let both = sync.clone().with_activity_collection(true);
    let saved = both
        .save_free(a, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(episodes(pool, a).await, 0);
    assert!(
        !report
            .book_returns_report(one.id, 90)
            .await
            .unwrap()
            .unwrap()
            .collection_healthy
    );
    assert!(
        report.returns_report(90).await.unwrap().collection_healthy,
        "book derivative failure closes only book coverage"
    );
    report.tick_book_collection(true).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION ci_book_coverage_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.ended_at IS NOT NULL THEN RAISE EXCEPTION 'fixture close failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER ci_book_coverage_fail BEFORE UPDATE ON reader_book_return_coverage FOR EACH ROW EXECUTE FUNCTION ci_book_coverage_fail();")
        .execute(&owner).await.unwrap();
    let failed = both
        .save_free(a, one.chapter, one.version, place(2), 1)
        .await;
    sqlx::raw_sql("DROP TRIGGER ci_book_coverage_fail ON reader_book_return_coverage; DROP FUNCTION ci_book_coverage_fail(); DROP TRIGGER ci_book_return_fail ON reader_book_return_subjects; DROP FUNCTION ci_book_return_fail();")
        .execute(&owner).await.unwrap();
    assert_eq!(
        failed,
        Err(SyncError::Unavailable),
        "progress cannot commit if missing coverage cannot be closed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM reader_book_positions WHERE user_id=$1 AND book_id=$2"
        )
        .bind(a)
        .bind(one.id)
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );
    sync.save_free(a, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(episodes(pool, a).await, 1);

    let b = actor(pool, "withdraw-race").await;
    actors.push(b);
    consent(pool, b, true).await;
    let mut withdrawal = pool.begin().await.unwrap();
    sqlx::query("SELECT user_id FROM rec_subjects WHERE user_id=$1 FOR UPDATE")
        .bind(b)
        .execute(&mut *withdrawal)
        .await
        .unwrap();
    let writer = sync.clone();
    let pending = tokio::spawn(async move {
        writer
            .save_free(b, one.chapter, one.version, place(1), 0)
            .await
    });
    sqlx::query("UPDATE rec_subjects SET enabled=false,enabled_at=NULL,subject_id=gen_random_uuid(),revision=revision+1 WHERE user_id=$1")
        .bind(b).execute(&mut *withdrawal).await.unwrap();
    withdrawal.commit().await.unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(episodes(pool, b).await, 0);

    let c = actor(pool, "finalize-withdraw").await;
    actors.push(c);
    consent(pool, c, true).await;
    cohort(pool, c, one.id, 31, [true; 2]).await;
    let mut withdrawal = pool.begin().await.unwrap();
    sqlx::query("SELECT user_id FROM rec_subjects WHERE user_id=$1 FOR UPDATE")
        .bind(c)
        .execute(&mut *withdrawal)
        .await
        .unwrap();
    report.tick_book_collection(false).await.unwrap();
    assert_eq!(
        episodes(pool, c).await,
        1,
        "finalizer skips the consent lock"
    );
    sqlx::query("UPDATE rec_subjects SET enabled=false,enabled_at=NULL,subject_id=gen_random_uuid(),revision=revision+1 WHERE user_id=$1")
        .bind(c).execute(&mut *withdrawal).await.unwrap();
    withdrawal.commit().await.unwrap();
    report.tick_book_collection(true).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reader_book_return_daily WHERE book_id=$1"
        )
        .bind(one.id)
        .fetch_one(pool)
        .await
        .unwrap(),
        0,
        "withdrawal before finalization never becomes anonymous counts"
    );
    owner.close().await;
}
