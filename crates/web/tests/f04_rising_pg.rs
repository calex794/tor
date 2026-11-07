//! F-04: real C-authoritative readers drive rising shelves, not views or bots.
//! Disposable tor_ci only: do not run against a retained PostgreSQL instance.
use sqlx::PgPool;
use std::time::{Duration, Instant};
use tor_reading::{
    repositories::PgReadingSync,
    services::progress::ReadingPlace,
};
use tor_search::{
    repositories::showcases::PgShowcases,
    services::showcases::RisingShowcases,
};

const BOOK: i64 = 99810;
const CHAPTER: i64 = 99820;
const SECOND_CHAPTER: i64 = 99821;
const VERSION: i64 = 99830;
const SECOND_VERSION: i64 = 99831;

async fn is_rising(showcase: &RisingShowcases<'_>) -> bool {
    showcase
        .rising_free_readers(12)
        .await
        .unwrap()
        .iter()
        .any(|book| book.id == BOOK as u64)
}

#[tokio::test]
#[ignore = "isolated tor_ci with genuine C completion/outbox facts"]
async fn f_04_rising_rejects_repeat_readers_owner_and_revoked_publication() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never write production database");

    // Only the C repository may create durable verified completion + outbox.
    // Fixed identities are reserved for this disposable CI fixture.
    for id in 99800_i64..=99806 {
        sqlx::query(
            "INSERT INTO users(id,email_normalized,display_name,password_hash)
             OVERRIDING SYSTEM VALUE VALUES($1,$2,$3,'$argon2id$fixture')",
        )
        .bind(id)
        .bind(format!("f04-rising-{id}@example.invalid"))
        .bind(format!("F04 Reader {id}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO books(id,owner_user_id,source_identity,target_language,title,
                           visibility,access_level)
         VALUES($1,99800,'ci:f04:rising:verified','ru','F04 Verified Rising',
                'public','basic')",
    )
    .bind(BOOK)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES
         (99820,$1,1,'Первая','public','free',0),
         (99821,$1,2,'Вторая','public','free',0)",
    )
    .bind(BOOK)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
         VALUES
         (99830,99820,1,E'Начало\n\nКонец😀'),
         (99831,99821,1,E'Начало\n\nКонец😀')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         VALUES(99820,99830),(99821,99831)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let c = PgReadingSync::new(pool.clone());
    let pg = PgShowcases::new(pool.clone());
    let shelf = RisingShowcases::new(&pg);
    let final_place = ReadingPlace {
        block_index: 2,
        char_offset: 7,
    };
    assert!(!is_rising(&shelf).await);

    // Two genuine completed readers are not a trend. Three distinct readers are.
    for user in [99801_i64, 99802] {
        c.save_free(user, CHAPTER, VERSION, final_place, 0)
            .await
            .unwrap();
    }
    assert!(!is_rising(&shelf).await);
    c.save_free(99803, CHAPTER, VERSION, final_place, 0)
        .await
        .unwrap();
    assert!(is_rising(&shelf).await);

    // Move three completed C facts into the older seven-day window. The
    // outbox remains immutable and authoritative; do not invent click counts.
    for user in [99804_i64, 99805, 99806] {
        c.save_free(user, CHAPTER, VERSION, final_place, 0)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE reader_chapter_completions
             SET completed_at=now()-interval '8 days'
             WHERE user_id=$1 AND book_id=$2",
        )
        .bind(user)
        .bind(BOOK)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert!(
        !is_rising(&shelf).await,
        "three current and three previous readers are not growing"
    );

    // One account reading another chapter cannot create a fourth reader.
    c.save_free(99801, SECOND_CHAPTER, SECOND_VERSION, final_place, 1)
        .await
        .unwrap();
    assert!(!is_rising(&shelf).await);

    // The author is not a reader for organic growth calculations.
    c.save_free(99800, CHAPTER, VERSION, final_place, 0)
        .await
        .unwrap();
    assert!(!is_rising(&shelf).await);

    // A revoked (suspended) former reader no longer counts in last-week
    // comparisons, and every public restriction is immediately reapplied.
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=99804")
        .execute(&pool)
        .await
        .unwrap();
    let start = Instant::now();
    assert!(is_rising(&shelf).await);
    let elapsed = start.elapsed();
    eprintln!("F04 rising PostgreSQL single-query sample: {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "even the small CI fixture must remain bounded"
    );

    for (sql, reset) in [
        (
            "UPDATE books SET access_level='adult' WHERE id=99810",
            "UPDATE books SET access_level='basic' WHERE id=99810",
        ),
        (
            "UPDATE books SET admin_hold=TRUE WHERE id=99810",
            "UPDATE books SET admin_hold=FALSE WHERE id=99810",
        ),
        (
            "UPDATE chapters SET admin_hold=TRUE WHERE id=99820",
            "UPDATE chapters SET admin_hold=FALSE WHERE id=99820",
        ),
        (
            "UPDATE books SET visibility='draft' WHERE id=99810",
            "UPDATE books SET visibility='public' WHERE id=99810",
        ),
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
        assert!(!is_rising(&shelf).await, "withdrawn Book is not a trend");
        sqlx::query(reset).execute(&pool).await.unwrap();
        assert!(is_rising(&shelf).await, "restored public Book can return");
    }
    // This test intentionally uses the one-time tor_ci disposable instance;
    // no data, tokens or events are projected to the running site.
}
