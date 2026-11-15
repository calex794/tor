//! One closed UTC window over matching C facts; no retired analytics relation is read.
#[path = "audience_pg/canonical/fixture.rs"]
mod fixture;
use fixture::Fixture;
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{AudienceError, ClosedReadingCounts},
};

async fn new_fixture() -> Fixture {
    let f = Fixture::new().await;
    f.sql(
        "CREATE TABLE reader_chapter_starts (LIKE public.reader_chapter_starts INCLUDING ALL);
        GRANT SELECT ON reader_chapter_starts TO tor_app",
    )
    .await;
    f.books(3).await;
    f
}
async fn start(f: &Fixture, user: i64, chapter: i64, at: &str, mode: &str) {
    assert!(["free", "paid", "invalid", "mismatch", "missing"].contains(&mode));
    let paid = matches!(mode, "paid" | "mismatch");
    let purchase = if paid { "123::bigint" } else { "NULL::bigint" };
    let authority = if mode == "invalid" {
        "untrusted"
    } else if paid {
        "C-verified-purchased-start"
    } else {
        "C-verified-free-start"
    };
    let extra = if mode == "paid" {
        "||jsonb_build_object('purchase_id',123)"
    } else if mode == "mismatch" {
        "||jsonb_build_object('purchase_id',999)"
    } else {
        ""
    };
    f.sql(&format!("WITH facts AS (
        INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,started_at,source_event_id,purchase_id)
        VALUES({user},1,{chapter},{chapter},{at},gen_random_uuid(),{purchase}) RETURNING *)
        INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
        SELECT source_event_id,'ChapterStartedV1',1,'chapter',chapter_id,source_event_id::text,
          jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,
            'version_id',first_version_id,'authority','{authority}'){extra},ARRAY['fixture-only']
        FROM facts WHERE {}",mode!="missing")).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and current migrations"]
async fn closed_c_windows_keep_chapter_starts_and_first_account_book_completions_distinct() {
    let f = new_fixture().await;
    let store = PgAudience::new(f.app.clone());
    let anchor = store.closed_report(1).await.unwrap().until_epoch;
    assert_eq!(anchor % 3600, 0);
    let at = |hours: i64| format!("to_timestamp(({anchor}+{hours}*3600)::double precision)");
    start(&f, 1, 11, &at(-2), "free").await;
    start(&f, 1, 12, &at(-3), "free").await;
    start(&f, 2, 21, &at(-36), "paid").await;
    start(&f, 3, 31, &at(-2), "invalid").await;
    start(&f, 4, 41, &at(-2), "mismatch").await;
    start(&f, 5, 51, &at(-2), "missing").await;
    start(&f, 6, 61, &at(1), "free").await;
    f.completion(1, 1, 11, &at(-2), "free").await;
    f.completion(1, 1, 12, &at(-1), "free").await;
    f.completion(2, 1, 21, &at(-200 * 24), "free").await;
    f.completion(2, 1, 22, &at(-2), "free").await;
    f.completion(3, 1, 31, &at(-200 * 24), "invalid").await;
    f.completion(3, 1, 32, &at(-3), "free").await;
    f.completion(4, 2, 41, &at(-36), "paid").await;
    f.completion(5, 2, 51, &at(-2), "mismatch").await;
    f.completion(6, 2, 61, &at(-2), "missing").await;
    f.completion(7, 3, 71, &at(1), "free").await;
    let report = store.closed_report(1).await.unwrap();
    assert_eq!(report.until_epoch % 3600, 0);
    assert_eq!(
        report.current,
        ClosedReadingCounts {
            starts: 2,
            completed_book_pairs: 2
        }
    );
    assert_eq!(
        report.previous,
        ClosedReadingCounts {
            starts: 1,
            completed_book_pairs: 1
        }
    );
    assert!(!format!("{report:?}").contains("Book 1"));
    for days in [1, 7, 30, 90] {
        let report = store.closed_report(days).await.unwrap();
        assert_eq!(report.days, days);
        assert_eq!(report.until_epoch % 3600, 0);
    }
    for days in [0, 2, 91] {
        assert_eq!(store.closed_report(days).await, Err(AudienceError::Invalid));
    }
    // Completion scope matches the existing canonical public-catalog report.
    // Historical authorized chapter starts do not disappear when a book is hidden.
    f.sql("DELETE FROM reader_return_public_books WHERE id=1")
        .await;
    let hidden = store.closed_report(1).await.unwrap();
    assert_eq!(
        hidden.current,
        ClosedReadingCounts {
            starts: 2,
            completed_book_pairs: 0
        }
    );
    assert_eq!(hidden.previous.completed_book_pairs, 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and current migrations"]
async fn completed_hour_edges_and_old_90_day_window_remain_read_only_and_exact() {
    let f = new_fixture().await;
    let store = PgAudience::new(f.app.clone());
    let anchor = store.closed_report(90).await.unwrap().until_epoch;
    // Well inside each side of the boundary; this regression is not a wall-clock race.
    let at = |hours: i64| format!("to_timestamp(({anchor}+{hours}*3600)::double precision)");
    start(&f, 1, 101, &at(-2), "free").await;
    start(&f, 2, 102, &at(-24 * 90 - 2), "free").await;
    start(&f, 3, 103, &at(-24 * 180 - 2), "free").await;
    f.completion(1, 1, 101, &at(-2), "free").await;
    f.completion(2, 2, 102, &at(-24 * 90 - 2), "paid").await;
    f.completion(3, 3, 103, &at(-24 * 180 - 2), "free").await;
    let report = store.closed_report(90).await.unwrap();
    assert_eq!(
        report.current,
        ClosedReadingCounts {
            starts: 1,
            completed_book_pairs: 1
        }
    );
    assert_eq!(
        report.previous,
        ClosedReadingCounts {
            starts: 1,
            completed_book_pairs: 1
        }
    );
    let mut lock = f.owner.begin().await.unwrap();
    sqlx::query(&format!(
        "LOCK TABLE {}.reader_chapter_starts IN ACCESS EXCLUSIVE MODE",
        f.schema
    ))
    .execute(&mut *lock)
    .await
    .unwrap();
    let began = std::time::Instant::now();
    assert_eq!(
        store.closed_report(90).await,
        Err(AudienceError::Unavailable)
    );
    assert!(began.elapsed() < std::time::Duration::from_secs(1));
    lock.rollback().await.unwrap();
    assert_eq!(
        store.closed_report(90).await.unwrap().current,
        report.current
    );
    f.finish().await;
}
