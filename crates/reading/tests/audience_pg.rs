//! FLOW: canonical C facts, exact whole-period uniqueness and adjacent UTC windows.
//! Run with the migrated disposable PostgreSQL database, never a persistent service.
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use tor_reading::{
    repositories::{audience::PgAudience, PgPreferredSendHour, PgReadingSync},
    services::{
        audience::{AudienceCounts, AudiencePeriod, AudienceReport},
        notification_send_time::HourConfidence,
        progress::ReadingPlace,
    },
};

struct Fixture {
    pool: PgPool,
    book: i64,
    users: [i64; 3],
    chapters: Vec<(i64, i64)>,
}

async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never test against production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = [0; 3];
    for (index, user) in users.iter_mut().enumerate() {
        *user = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Audience reader','$argon2id$fixture') RETURNING id",
        )
        .bind(format!("audience-{nonce}-{index}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
    }
    let book = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Private analytics title','public','basic') RETURNING id",
    )
    .bind(format!("ci:audience:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapters = sqlx::query(
        "WITH added AS (
           INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
           SELECT $1,n,'Chapter','public','free',0 FROM generate_series(1,127) AS n
           RETURNING id,chapter_no
         ), versions AS (
           INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
           SELECT id,1,'read me' FROM added RETURNING chapter_id,id
         ), publications AS (
           INSERT INTO chapter_publications(chapter_id,version_id)
           SELECT chapter_id,id FROM versions
         ) SELECT v.chapter_id,v.id FROM versions v
           JOIN added c ON c.id=v.chapter_id ORDER BY c.chapter_no",
    )
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.get("chapter_id"), row.get("id")))
    .collect();
    Fixture {
        pool,
        book,
        users,
        chapters,
    }
}

/// Fixture C facts and matching committed events. The production writer has
/// separate concurrency/ACL tests; this exercises the actual aggregate SQL.
async fn events(f: &Fixture, user: i64, range: (i32, i32), hours_ago: i64, finish: bool) {
    let (table, time, event_type, authority) = if finish {
        (
            "reader_chapter_completions",
            "completed_at",
            "ChapterCompletedV1",
            "C-verified-free-position",
        )
    } else {
        (
            "reader_chapter_starts",
            "started_at",
            "ChapterStartedV1",
            "C-verified-free-start",
        )
    };
    // Identifiers above are fixed test constants, never input from an HTTP request.
    let sql = format!(
        "WITH facts AS (
           INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,{time},source_event_id)
           SELECT $1,$2,c.id,p.version_id,now()-$5::bigint*interval '1 hour',gen_random_uuid()
           FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
           WHERE c.book_id=$2 AND c.chapter_no BETWEEN $3 AND $4
           ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING RETURNING *
         ) INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,
             aggregate_id,dedupe_key,payload,required_consumers)
         SELECT source_event_id,'{event_type}',1,'chapter',chapter_id,
             source_event_id::text,
             jsonb_build_object('user_id',user_id,'book_id',book_id,
                 'chapter_id',chapter_id,'version_id',first_version_id,
                 'authority','{authority}'),ARRAY['audience-fixture-only'] FROM facts"
    );
    sqlx::query(&sql)
        .bind(user)
        .bind(f.book)
        .bind(range.0)
        .bind(range.1)
        .bind(hours_ago)
        .execute(&f.pool)
        .await
        .unwrap();
}

fn increase(before: &AudiencePeriod, after: &AudiencePeriod) -> AudienceCounts {
    assert!(before.available && after.available);
    let before = before.counts.unwrap_or_default();
    let after = after.counts.unwrap_or_default();
    AudienceCounts {
        starts: after.starts - before.starts,
        completions: after.completions - before.completions,
        unique_readers: after.unique_readers - before.unique_readers,
        unique_started: after.unique_started - before.unique_started,
        unique_completed: after.unique_completed - before.unique_completed,
    }
}

fn counts(
    starts: i64,
    completions: i64,
    readers: i64,
    started: i64,
    completed: i64,
) -> AudienceCounts {
    AudienceCounts {
        starts,
        completions,
        unique_readers: readers,
        unique_started: started,
        unique_completed: completed,
    }
}

fn intervals(report: &AudienceReport) {
    assert_eq!(report.measured_at_utc, report.current.until_utc);
    assert_eq!(report.current.from_utc, report.previous.until_utc);
    for period in [&report.current, &report.previous] {
        let from = DateTime::parse_from_rfc3339(&period.from_utc).unwrap();
        let until = DateTime::parse_from_rfc3339(&period.until_utc).unwrap();
        assert_eq!(from.offset().local_minus_utc(), 0);
        assert_eq!(until.offset().local_minus_utc(), 0);
        assert_eq!(
            (until - from).num_seconds(),
            i64::from(report.days) * 86_400
        );
        assert!(until <= Utc::now());
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn flow_reading_periods_count_accounts_once_without_truncating_or_rebuilding_profiles() {
    let f = fixture().await;
    assert_eq!(f.chapters.len(), 127);
    let repo = PgAudience::new(f.pool.clone());
    let mut before = Vec::new();
    for days in [1, 7, 30, 90] {
        before.push(repo.report(days).await.unwrap());
    }
    // More than the old detailed-report LIMIT 100, spread over different dates.
    // No REC subject or consented profile exists: these are C's canonical facts.
    for finish in [false, true] {
        events(&f, f.users[0], (1, 120), 36, finish).await;
        events(&f, f.users[0], (126, 126), -1, finish).await; // future excluded
    }
    // Exercise the real C writer as well as aggregate fixtures.
    let (chapter, version) = f.chapters[120];
    PgReadingSync::new(f.pool.clone())
        .save_free(
            f.users[0],
            chapter,
            version,
            ReadingPlace {
                block_index: 1,
                char_offset: 7,
            },
            0,
        )
        .await
        .unwrap();
    events(&f, f.users[1], (122, 122), 72, true).await;
    events(&f, f.users[0], (123, 123), 8 * 24, false).await;
    events(&f, f.users[2], (124, 124), 45 * 24, true).await;
    events(&f, f.users[2], (125, 125), 120 * 24, false).await;

    // A private smart-hour estimate consumes only verified C facts; it must
    // reject a forged authority, stale first-read and another user's activity.
    let hour_repo = PgPreferredSendHour::new(f.pool.clone());
    let preferred = hour_repo.estimate(f.users[0]).await.unwrap();
    assert_eq!(preferred.confidence, HourConfidence::High);
    assert_eq!(preferred.observed_starts, 122);
    assert_eq!(
        hour_repo.estimate(f.users[1]).await.unwrap().confidence,
        HourConfidence::None
    );
    assert_eq!(
        hour_repo.estimate(0).await,
        Err(tor_reading::repositories::PreferenceError::Invalid)
    );

    // Re-delivery cannot duplicate the canonical row or create another account.
    events(&f, f.users[0], (1, 120), 36, true).await;
    // A fabricated source on a C-shaped fixture must not become trusted evidence.
    events(&f, f.users[0], (127, 127), 12, false).await;
    sqlx::query(
        "UPDATE outbox_events e SET payload=jsonb_set(payload,'{authority}','\"ClientClaim\"')
         FROM reader_chapter_starts s JOIN chapters c ON c.id=s.chapter_id
         WHERE e.id=s.source_event_id AND s.book_id=$1 AND c.chapter_no=127",
    )
    .bind(f.book)
    .execute(&f.pool)
    .await
    .unwrap();
    let expected = [
        (counts(1, 1, 1, 1, 1), counts(120, 120, 1, 1, 1)),
        (counts(121, 122, 2, 1, 2), counts(1, 0, 1, 1, 0)),
        (counts(122, 122, 2, 1, 2), counts(0, 1, 1, 0, 1)),
        (counts(122, 123, 3, 1, 3), counts(1, 0, 1, 1, 0)),
    ];
    for (baseline, (current, previous)) in before.iter().zip(expected) {
        let report = repo.report(baseline.days).await.unwrap();
        intervals(&report);
        assert_eq!(increase(&baseline.current, &report.current), current);
        assert_eq!(increase(&baseline.previous, &report.previous), previous);
        assert!(!format!("{report:?}").contains("Private analytics title"));
    }
    // Moderation changes current access, not the historical authorized C fact.
    let before_hold = repo.report(7).await.unwrap();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    let held = repo.report(7).await.unwrap();
    assert_eq!(before_hold.current.counts, held.current.counts);
    assert_eq!(before_hold.previous.counts, held.previous.counts);

    // Existing account-erasure cascade removes identities from these facts.
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(f.users.to_vec())
        .execute(&f.pool)
        .await
        .unwrap();
    let erased = repo.report(90).await.unwrap();
    assert_eq!(erased.current.counts, before[3].current.counts);
    assert_eq!(erased.previous.counts, before[3].previous.counts);
    sqlx::query("DELETE FROM outbox_events WHERE payload->>'book_id'=$1")
        .bind(f.book.to_string())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    f.pool.close().await;
}
