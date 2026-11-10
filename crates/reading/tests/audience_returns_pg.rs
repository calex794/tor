//! FLOW: real C writers, consent erasure, exact UTC returns and bounded history.
//! Only the disposable migrated tor_ci database is accepted.
use sqlx::PgPool;
use tor_reading::{
    repositories::{
        returns::PgReadingReturns, sync::PurchasedFinish, PgLibrary, PgReadingProgress,
        PgReadingSync,
    },
    services::{
        progress::{ProgressError, ReadingPlace},
        sync::SyncError,
    },
};

#[derive(Clone, Copy)]
struct Book {
    id: i64,
    chapter: i64,
    version: i64,
}

fn place(offset: i32) -> ReadingPlace {
    ReadingPlace {
        block_index: 1,
        char_offset: offset,
    }
}

async fn actor(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Return fixture','$argon2id$fixture') RETURNING id")
        .bind(format!("returns-{name}-{}@example.invalid", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
        .fetch_one(pool).await.unwrap()
}

async fn consent(pool: &PgPool, actor: i64, enabled: bool) -> String {
    sqlx::query_scalar(
        "INSERT INTO rec_subjects(user_id,enabled,enabled_at)
         VALUES($1,$2,CASE WHEN $2 THEN now() ELSE NULL END)
         ON CONFLICT(user_id) DO UPDATE SET subject_id=gen_random_uuid(),
           enabled=$2,enabled_at=CASE WHEN $2 THEN now() ELSE NULL END,revision=rec_subjects.revision+1
         RETURNING subject_id::text",
    ).bind(actor).bind(enabled).fetch_one(pool).await.unwrap()
}

async fn book(pool: &PgPool, name: &str, paid: bool) -> Book {
    let id = sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES($1,'ru','Return fixture','public','basic') RETURNING id")
        .bind(format!("ci:returns:{name}:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
        .fetch_one(pool).await.unwrap();
    let chapter = sqlx::query_scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES($1,1,'Chapter','public',$2,$3) RETURNING id")
        .bind(id).bind(if paid {"paid"} else {"free"}).bind(if paid {100_i64} else {0})
        .fetch_one(pool).await.unwrap();
    let version = sqlx::query_scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES($1,1,'abcdefghij') RETURNING id")
        .bind(chapter).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    Book {
        id,
        chapter,
        version,
    }
}

async fn raw(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reader_activity_days d JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(actor).fetch_one(pool).await.unwrap()
}

async fn enrolled(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reader_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(actor).fetch_one(pool).await.unwrap()
}

async fn purchase(pool: &PgPool, actor: i64, book: Book) -> i64 {
    sqlx::query_scalar("INSERT INTO chapter_purchase_facts(user_id,chapter_id,version_id,settlement_reference,amount_minor,currency) VALUES($1,$2,$3,$4,100,'RUB') RETURNING id")
        .bind(actor).bind(book.chapter).bind(book.version).bind(format!("ci:return:{actor}:{}", book.chapter))
        .fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn flow_returns_confirmed_progress_privacy_and_exact_utc_cohorts() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never test a permanent database");
    let pool = PgPool::connect(&url).await.unwrap();
    let coverage_before: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(id),0) FROM reader_return_coverage")
            .fetch_one(&pool)
            .await
            .unwrap();
    let report = PgReadingReturns::new(pool.clone());
    report.start_collection(true).await.unwrap();
    let sync = PgReadingSync::new(pool.clone()).with_activity_collection(true);
    let mut actors = Vec::new();
    writer_cases(&pool, &report, &sync, &mut actors).await;
    paid_and_archived_cases(&pool, &sync, &mut actors).await;
    failure_and_consent_race(&pool, &report, &sync, &mut actors).await;
    cohort_cases(&pool, &report, &sync, &mut actors).await;
    for actor in actors {
        consent(&pool, actor, false).await;
    }
    report.tick_collection(false).await.unwrap();
    sqlx::query("DELETE FROM reader_return_coverage WHERE id>$1 AND NOT EXISTS(SELECT 1 FROM reader_return_subjects s WHERE s.coverage_epoch_id=reader_return_coverage.id)")
        .bind(coverage_before).execute(&pool).await.unwrap();
    pool.close().await;
}

async fn writer_cases(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
) {
    let a = actor(pool, "free").await;
    actors.push(a);
    let one = book(pool, "free", false).await;
    sync.save_free(a, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(raw(pool, a).await, 0, "default consent is off");
    consent(pool, a, true).await;
    sync.save_free(a, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(raw(pool, a).await, 1);
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
    sync.save_free(a, one.chapter, one.version, place(3), 2)
        .await
        .unwrap();
    assert_eq!(raw(pool, a).await, 1, "same UTC day/book cannot multiply");
    let two = book(pool, "second-book", false).await;
    sync.save_free(a, two.chapter, two.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(raw(pool, a).await, 2);
    assert_eq!(
        enrolled(pool, a).await,
        1,
        "different books share one global cohort"
    );
    let first = report.returns_report(7).await.unwrap();
    let again = PgReadingReturns::new(pool.clone())
        .returns_report(7)
        .await
        .unwrap();
    assert_eq!(
        first.current, again.current,
        "reopening the repository preserves counts"
    );

    let b = actor(pool, "manual").await;
    actors.push(b);
    consent(pool, b, true).await;
    sync.save_free(b, one.chapter, one.version, place(0), 0)
        .await
        .unwrap();
    assert_eq!(enrolled(pool, b).await, 0, "initial anchor is not reading");
    let manual = PgReadingProgress::new(pool.clone()).with_activity_collection(true);
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
    assert_eq!(raw(pool, b).await, 1);
    let old: String =
        sqlx::query_scalar("SELECT subject_id::text FROM rec_subjects WHERE user_id=$1")
            .bind(b)
            .fetch_one(pool)
            .await
            .unwrap();
    consent(pool, b, false).await;
    let erased: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM reader_activity_days WHERE subject_id=$1::uuid)+(SELECT count(*) FROM reader_return_subjects WHERE subject_id=$1::uuid)")
        .bind(old).fetch_one(pool).await.unwrap();
    assert_eq!(
        erased, 0,
        "withdrawal erases details and enrollment atomically"
    );
    consent(pool, b, true).await;
    manual
        .save_position(b, one.chapter, one.version, place(2))
        .await
        .unwrap();
    assert_eq!(
        enrolled(pool, b).await,
        0,
        "successful manual no-op cannot backfill a new consent epoch"
    );
    manual
        .save_position(b, one.chapter, one.version, place(3))
        .await
        .unwrap();
    assert_eq!(enrolled(pool, b).await, 1);

    let c = actor(pool, "devices").await;
    actors.push(c);
    consent(pool, c, true).await;
    let (left, right) = tokio::join!(
        sync.save_free(c, one.chapter, one.version, place(1), 0),
        sync.save_free(c, one.chapter, one.version, place(2), 0)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(raw(pool, c).await, 1);
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(one.chapter)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_free(c, one.chapter, one.version, place(3), 1)
            .await,
        Err(SyncError::Conflict)
    );
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(one.chapter)
        .execute(pool)
        .await
        .unwrap();
}

async fn paid_and_archived_cases(pool: &PgPool, sync: &PgReadingSync, actors: &mut Vec<i64>) {
    let buyer = actor(pool, "paid").await;
    actors.push(buyer);
    let stranger = actor(pool, "stranger").await;
    actors.push(stranger);
    consent(pool, buyer, true).await;
    consent(pool, stranger, true).await;
    let paid = book(pool, "paid", true).await;
    let paid_id = purchase(pool, buyer, paid).await;
    sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(2), 0)
        .await
        .unwrap();
    assert_eq!(raw(pool, buyer).await, 1);
    assert_eq!(
        sync.save_purchased(stranger, paid_id, paid.chapter, paid.version, place(3), 0)
            .await,
        Err(SyncError::NotFound)
    );
    sqlx::query("INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) VALUES($1,$2,'refund')")
        .bind(paid_id).bind(format!("ci:return-refund:{paid_id}")).execute(pool).await.unwrap();
    assert_eq!(
        sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(3), 1)
            .await,
        Err(SyncError::NotFound)
    );

    let archived = book(pool, "archived", false).await;
    let archive_reader = actor(pool, "archive").await;
    actors.push(archive_reader);
    consent(pool, archive_reader, true).await;
    PgLibrary::new(pool.clone())
        .add_book(archive_reader, archived.id)
        .await
        .unwrap();
    let mut digest = [0_u8; 32];
    digest[..8].copy_from_slice(&archive_reader.to_le_bytes());
    digest[31] = 19;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(archive_reader)
    .bind(digest.as_slice())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(archived.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='ordinary_deleted',ordinary_deleted_at=now(),visibility='draft' WHERE id=$1")
        .bind(archived.id).execute(pool).await.unwrap();
    sync.save_archived_free(
        archive_reader,
        archived.chapter,
        archived.version,
        place(2),
        0,
        &digest,
    )
    .await
    .unwrap();
    assert_eq!(
        raw(pool, archive_reader).await,
        1,
        "allowed pinned reading is observed without public metadata"
    );
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1")
            .bind(archive_reader)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        starts, 0,
        "archive day does not invent canonical first-start events"
    );
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_archived_free(
            archive_reader,
            archived.chapter,
            archived.version,
            place(3),
            1,
            &digest
        )
        .await,
        Err(SyncError::NotFound)
    );
    encrypted_case(pool, sync, actors).await;
}

async fn encrypted_case(pool: &PgPool, sync: &PgReadingSync, actors: &mut Vec<i64>) {
    let buyer = actor(pool, "encrypted").await;
    actors.push(buyer);
    consent(pool, buyer, true).await;
    let paid = book(pool, "encrypted", true).await;
    let object: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects(id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES($1::uuid,'chapter','tor-private-return-ci','private/chapter-versions/'||$1::text||'.enc','attached',64,
           decode(repeat('11',32),'hex'),sha256(convert_to('abcdefghij','UTF8')),decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),convert_to(concat('tor.encrypted.chapter.v1',chr(10),$2::text,chr(10),$3::text,chr(10),$1::text,chr(10),encode(sha256(convert_to('abcdefghij','UTF8')),'hex'),chr(10)),'UTF8'),'gzip',now(),now())",
    ).bind(&object).bind(paid.chapter).bind(paid.version).execute(pool).await.unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(paid.version)
        .bind(&object)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chapter_reader_manifests(chapter_id,version_id,storage_object_id,content_sha256,block_utf16_lengths,last_readable_block_index,last_readable_char_offset) VALUES($1,$2,$3::uuid,sha256(convert_to('abcdefghij','UTF8')),ARRAY[10],1,10)")
        .bind(paid.chapter).bind(paid.version).bind(&object).execute(pool).await.unwrap();
    let hash: String =
        sqlx::query_scalar("SELECT encode(sha256(convert_to('abcdefghij','UTF8')),'hex')")
            .fetch_one(pool)
            .await
            .unwrap();
    let paid_id = purchase(pool, buyer, paid).await;
    sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        raw(pool, buyer).await,
        0,
        "encrypted shadow position alone is not verified reading"
    );
    let command = || PurchasedFinish {
        purchase_id: paid_id,
        chapter_id: paid.chapter,
        version_id: paid.version,
        place: place(10),
        expected_revision: 1,
        plaintext_sha256: &hash,
    };
    assert!(
        sync.complete_purchased_s3(buyer, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(raw(pool, buyer).await, 1);
    consent(pool, buyer, false).await;
    consent(pool, buyer, true).await;
    assert!(
        !sync
            .complete_purchased_s3(buyer, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(
        raw(pool, buyer).await,
        0,
        "replayed encrypted completion cannot create a new consented day"
    );
}

async fn cohort(pool: &PgPool, actor: i64, age: i32, coverage: i64, flags: [bool; 3]) {
    sqlx::query(
        "INSERT INTO reader_return_subjects(subject_id,cohort_day,coverage_epoch_id,returned_d1,returned_d7,returned_d30)
         SELECT subject_id,(now() AT TIME ZONE 'UTC')::date-$2::int,$3,$4,$5,$6
         FROM rec_subjects WHERE user_id=$1",
    ).bind(actor).bind(age).bind(coverage).bind(flags[0]).bind(flags[1]).bind(flags[2]).execute(pool).await.unwrap();
}

async fn active_coverage(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM reader_return_coverage WHERE ended_at IS NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn failure_and_consent_race(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
) {
    // Fault injection is test-only DDL. Real C operations continue to use the
    // restricted application role; never grant it schema creation privileges.
    let owner_url = std::env::var("MIGRATOR_URL").expect("disposable CI schema owner");
    assert!(owner_url.ends_with("/tor_ci") && owner_url.contains("@127.0.0.1:"));
    let owner = PgPool::connect(&owner_url).await.unwrap();
    let identity: (String, String) = sqlx::query_as("SELECT current_database(),current_user")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(identity, ("tor_ci".into(), "tor_migrator".into()));
    let a = actor(pool, "fault").await;
    actors.push(a);
    consent(pool, a, true).await;
    let one = book(pool, "fault", false).await;
    let coverage = active_coverage(pool).await;
    cohort(pool, a, 1, coverage, [false; 3]).await;
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION ci_reader_return_fail() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN IF NEW.book_id={} THEN RAISE EXCEPTION 'fixture derivative failure'; END IF; RETURN NEW; END $$;
         CREATE TRIGGER ci_reader_return_fail BEFORE INSERT ON reader_activity_days
         FOR EACH ROW EXECUTE FUNCTION ci_reader_return_fail();", one.id
    )).execute(&owner).await.unwrap();
    let saved = sync
        .save_free(a, one.chapter, one.version, place(2), 0)
        .await
        .unwrap();
    sqlx::raw_sql("DROP TRIGGER ci_reader_return_fail ON reader_activity_days; DROP FUNCTION ci_reader_return_fail();")
        .execute(&owner).await.unwrap();
    assert_eq!(
        saved.revision, 1,
        "an optional derivative failure does not discard accepted progress"
    );
    assert_eq!(raw(pool, a).await, 0);
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_return_coverage WHERE ended_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(active, 0, "the missing day closes coverage durably");
    assert!(!report.returns_report(7).await.unwrap().collection_healthy);
    consent(pool, a, false).await;
    report.tick_collection(true).await.unwrap();

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
    // The held consent row orders this operation before capture, regardless of
    // whether the writer has reached the row lock by the time we commit.
    sqlx::query("UPDATE rec_subjects SET enabled=false,enabled_at=NULL,subject_id=gen_random_uuid(),revision=revision+1 WHERE user_id=$1")
        .bind(b).execute(&mut *withdrawal).await.unwrap();
    withdrawal.commit().await.unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(enrolled(pool, b).await, 0);
    assert_eq!(raw(pool, b).await, 0);

    let c = actor(pool, "finalize-withdraw").await;
    actors.push(c);
    consent(pool, c, true).await;
    cohort(pool, c, 31, active_coverage(pool).await, [true; 3]).await;
    let before: i64 = sqlx::query_scalar("SELECT COALESCE(sum(enrolled),0)::bigint FROM reader_return_daily WHERE cohort_day=(now() AT TIME ZONE 'UTC')::date-31")
        .fetch_one(pool).await.unwrap();
    let mut withdrawal = pool.begin().await.unwrap();
    sqlx::query("SELECT user_id FROM rec_subjects WHERE user_id=$1 FOR UPDATE")
        .bind(c)
        .execute(&mut *withdrawal)
        .await
        .unwrap();
    // Finalization must SKIP the locked F consent row, not anonymize around it.
    report.tick_collection(false).await.unwrap();
    sqlx::query("UPDATE rec_subjects SET enabled=false,enabled_at=NULL,subject_id=gen_random_uuid(),revision=revision+1 WHERE user_id=$1")
        .bind(c).execute(&mut *withdrawal).await.unwrap();
    withdrawal.commit().await.unwrap();
    report.tick_collection(true).await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT COALESCE(sum(enrolled),0)::bigint FROM reader_return_daily WHERE cohort_day=(now() AT TIME ZONE 'UTC')::date-31")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        before, after,
        "withdrawal wins before the finalization lock"
    );
}

async fn cohort_cases(
    pool: &PgPool,
    report: &PgReadingReturns,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
) {
    let coverage = active_coverage(pool).await;
    // This is temporal test evidence only. Production never changes the start
    // of an epoch or backfills a day from canonical first-start/latest-position.
    sqlx::query("UPDATE reader_return_coverage SET started_at=((now() AT TIME ZONE 'UTC')::date-1)::timestamp AT TIME ZONE 'UTC',last_heartbeat_at=now() WHERE id=$1")
        .bind(coverage).execute(pool).await.unwrap();
    let before = report.returns_report(90).await.unwrap();
    let cases = [
        (2, [true, false, false]),
        (8, [true, true, false]),
        (31, [false, false, true]),
        (2, [false; 3]),
        (1, [true; 3]),
        (4, [true; 3]),
    ];
    let mut subjects = Vec::new();
    for (index, (age, flags)) in cases.into_iter().enumerate() {
        let a = actor(pool, &format!("cohort-{index}")).await;
        actors.push(a);
        subjects.push(a);
        consent(pool, a, true).await;
        cohort(pool, a, age, coverage, flags).await;
    }
    // More than a detailed-report LIMIT 100: the query must count the entire
    // period, not a page of readers or a sum of unrelated chapter first facts.
    for index in 0..101 {
        let a = actor(pool, &format!("wide-{index}")).await;
        actors.push(a);
        consent(pool, a, true).await;
        cohort(pool, a, 2, coverage, [true, false, false]).await;
    }
    let after = report.returns_report(90).await.unwrap();
    assert_eq!(after.current.enrolled - before.current.enrolled, 107);
    let one = after.current.metrics[0];
    let old = before.current.metrics[0];
    assert_eq!(
        (
            one.eligible - old.eligible,
            one.returned - old.returned,
            one.pending - old.pending,
            one.unknown - old.unknown
        ),
        (103, 102, 1, 3)
    );
    let seven = after.current.metrics[1];
    let old = before.current.metrics[1];
    assert_eq!(
        (
            seven.eligible - old.eligible,
            seven.returned - old.returned,
            seven.pending - old.pending,
            seven.unknown - old.unknown
        ),
        (1, 1, 105, 1)
    );
    let thirty = after.current.metrics[2];
    let old = before.current.metrics[2];
    assert_eq!(
        (
            thirty.eligible - old.eligible,
            thirty.returned - old.returned,
            thirty.pending - old.pending,
            thirty.unknown - old.unknown
        ),
        (1, 1, 106, 0)
    );
    for days in [1, 7, 30, 90] {
        let result = report.returns_report(days).await.unwrap();
        assert_eq!(result.current.from_utc, result.previous.until_utc);
        for period in [&result.current, &result.previous] {
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

    let finished = subjects[2];
    let one_book = book(pool, "retention", false).await;
    // A recent D30 detail is removed at cohort finalization; an unrelated
    // expired detail is independently removed at the strict 30*24h boundary.
    sqlx::query("INSERT INTO reader_activity_days(subject_id,book_id,activity_day,first_seen_at) SELECT subject_id,$2,(now() AT TIME ZONE 'UTC')::date,now() FROM rec_subjects WHERE user_id=$1")
        .bind(finished).bind(one_book.id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO reader_activity_days(subject_id,book_id,activity_day,first_seen_at) SELECT subject_id,$2,((now()-interval '31 days') AT TIME ZONE 'UTC')::date,now()-interval '31 days' FROM rec_subjects WHERE user_id=$1")
        .bind(subjects[0]).bind(one_book.id).execute(pool).await.unwrap();
    report.tick_collection(true).await.unwrap();
    assert_eq!(
        raw(pool, finished).await,
        0,
        "finalization erases all no-longer-needed personal day detail"
    );
    assert_eq!(
        raw(pool, subjects[0]).await,
        0,
        "raw detail expires in thirty days"
    );
    let marker: bool = sqlx::query_scalar("SELECT r.enrolled AND r.cohort_day IS NULL AND r.coverage_epoch_id IS NULL AND r.returned_d1 IS NULL AND r.returned_d7 IS NULL AND r.returned_d30 IS NULL FROM reader_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(finished).fetch_one(pool).await.unwrap();
    assert!(marker);
    let summarized = report.returns_report(90).await.unwrap();
    assert_eq!(
        after.current, summarized.current,
        "anonymization does not change report totals"
    );
    report.tick_collection(true).await.unwrap();
    assert_eq!(
        summarized.current,
        report.returns_report(90).await.unwrap().current,
        "repeated finalization does not double count"
    );
    sync.save_free(finished, one_book.chapter, one_book.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        raw(pool, finished).await,
        0,
        "a completed enrollment cannot begin collecting forever"
    );
    let still_marker: bool = sqlx::query_scalar("SELECT cohort_day IS NULL FROM reader_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(finished).fetch_one(pool).await.unwrap();
    assert!(
        still_marker,
        "the same consent epoch must not be called new again"
    );

    report.start_collection(true).await.unwrap();
    let gap = report.returns_report(90).await.unwrap();
    assert_eq!(
        gap.current.metrics[0].eligible, summarized.current.metrics[0].eligible,
        "yesterday's complete day survives today's restart"
    );
    assert_eq!(
        gap.current.metrics[0].unknown,
        summarized.current.metrics[0].unknown + 1,
        "today's irreversible gap is unknown, not zero"
    );
    let cross_epoch = actor(pool, "cross-coverage").await;
    actors.push(cross_epoch);
    consent(pool, cross_epoch, true).await;
    cohort(pool, cross_epoch, 1, coverage, [false; 3]).await;
    sync.save_free(cross_epoch, one_book.chapter, one_book.version, place(1), 0)
        .await
        .unwrap();
    let returned: bool = sqlx::query_scalar("SELECT returned_d1 FROM reader_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(cross_epoch).fetch_one(pool).await.unwrap();
    assert!(
        returned,
        "an accepted return can arrive in a new coverage epoch under the same consent"
    );
    assert!(report.returns_report(90).await.unwrap().collection_healthy);
    assert!(
        !report
            .returns_report(90)
            .await
            .unwrap()
            .current
            .coverage_complete
    );
    // Restore the anonymous test contribution only, retaining any pre-existing
    // fixtures. Every chosen subject was in this test's single age-31 cohort.
    sqlx::query("UPDATE reader_return_daily SET enrolled=enrolled-1,eligible=eligible-CASE WHEN day=30 THEN 1 ELSE 0 END,returned=returned-CASE WHEN day=30 THEN 1 ELSE 0 END,unknown=unknown-CASE WHEN day=30 THEN 0 ELSE 1 END WHERE cohort_day=(now() AT TIME ZONE 'UTC')::date-31")
        .execute(pool).await.unwrap();
    sqlx::query("DELETE FROM reader_return_daily WHERE enrolled=0 AND cohort_day=(now() AT TIME ZONE 'UTC')::date-31")
        .execute(pool).await.unwrap();
}
