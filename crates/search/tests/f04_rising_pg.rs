//! F-04: validate public selection and revocation with synthetic C-shaped facts.
//! Disposable tor_ci only: do not run against a retained PostgreSQL instance.
use sqlx::PgPool;
use std::time::{Duration, Instant};
use tor_search::{repositories::showcases::PgShowcases, services::showcases::RisingShowcases};

const BOOK: i64 = 99810;
const CHAPTER: i64 = 99820;
const SECOND_CHAPTER: i64 = 99821;
const VERSION: i64 = 99830;
const SECOND_VERSION: i64 = 99831;

async fn insert_synthetic_completion(pool: &PgPool, user: i64, chapter: i64, version: i64) {
    sqlx::query(
        "WITH committed AS (
             INSERT INTO reader_chapter_completions
                 (user_id,book_id,chapter_id,first_version_id,source_event_id)
             VALUES ($1,$2,$3,$4,gen_random_uuid())
             RETURNING user_id,book_id,chapter_id,first_version_id,source_event_id
         )
         INSERT INTO outbox_events (
             id,event_type,schema_version,aggregate_kind,aggregate_id,
             dedupe_key,payload,required_consumers
         )
         SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,
                concat('chapter-completed:',user_id,':',chapter_id),
                jsonb_build_object('user_id',user_id,'book_id',book_id,
                    'chapter_id',chapter_id,'version_id',first_version_id,
                    'authority','C-verified-free-position'),
                ARRAY['ranking-eligibility-v1','achievements-v1']
         FROM committed",
    )
    .bind(user)
    .bind(BOOK)
    .bind(chapter)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
}

async fn is_rising(showcase: &RisingShowcases<'_>) -> bool {
    showcase
        .rising_free_readers(12)
        .await
        .unwrap()
        .iter()
        .any(|book| book.id == BOOK as u64)
}

#[tokio::test]
#[ignore = "isolated tor_ci with synthetic completion/outbox fixture"]
async fn f_04_rising_rejects_repeat_readers_owner_and_revoked_publication() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never write production database");

    // Synthetic fixture emulates C event shape; real C atomic provenance is
    // independently tested by tor-reading/completion_pg.
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

    let pg = PgShowcases::new(pool.clone());
    let shelf = RisingShowcases::new(&pg);
    assert!(!is_rising(&shelf).await);

    // Two distinct readers are not a trend. Three distinct readers are.
    for user in [99801_i64, 99802] {
        insert_synthetic_completion(&pool, user, CHAPTER, VERSION).await;
    }
    assert!(!is_rising(&shelf).await);
    insert_synthetic_completion(&pool, 99803, CHAPTER, VERSION).await;
    assert!(is_rising(&shelf).await);

    // Move three synthetic facts into the older seven-day window.
    // The corresponding outbox remains unchanged; no click counts.
    for user in [99804_i64, 99805, 99806] {
        insert_synthetic_completion(&pool, user, CHAPTER, VERSION).await;
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
    insert_synthetic_completion(&pool, 99801, SECOND_CHAPTER, SECOND_VERSION).await;
    assert!(!is_rising(&shelf).await);

    // The author is not a reader for organic growth calculations.
    insert_synthetic_completion(&pool, 99800, CHAPTER, VERSION).await;
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
            "UPDATE chapters SET admin_hold=TRUE WHERE id IN (99820,99821)",
            "UPDATE chapters SET admin_hold=FALSE WHERE id IN (99820,99821)",
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
