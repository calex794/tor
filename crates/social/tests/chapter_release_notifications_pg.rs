//! S058: ChapterPublished root fans out once to recipient-scoped E-08 events.
//! Only disposable /tor_ci PostgreSQL may be modified by this test.
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgPool};
use tor_social::repositories::notifications::PgNotifications;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_release_fanout_is_bounded_idempotent_and_respects_preferences() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never access production");
    let mut lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut lock)
        .await
        .unwrap();
    // Fixture IDs use a disposable high range rather than setval(), which
    // the least-privileged tor_app role is correctly forbidden to call.
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let mut users = Vec::new();
    for role in [
        "author",
        "follower",
        "opted-out",
        "late-follower",
        "library-only",
    ] {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(id,email_normalized,display_name,password_hash)
             VALUES((SELECT GREATEST(COALESCE(MAX(id),0),1000000000000::bigint)+1 FROM users),
                    $1,$2,'$argon2id$test') RETURNING id",
        )
        .bind(format!("s058-release-{role}-{nonce}@example.invalid"))
        .bind(format!("S058 {role}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(user);
    }
    let (author, follower, opted_out, late_follower, library_only) =
        (users[0], users[1], users[2], users[3], users[4]);
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru',$2,'public',$3,'basic') RETURNING id",
    )
    .bind(format!("ci:s058-fanout:{nonce}"))
    .bind(format!("Chapter fanout {nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$3),($2,$3)")
        .bind(follower)
        .bind(opted_out)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    // NEwBOO sends chapter-created notifications to library readers.
    // Also save the same book for a follower to verify union deduplication.
    sqlx::query(
        "INSERT INTO reader_library_items(user_id,book_id)
         VALUES($1,$3),($2,$3)",
    )
    .bind(follower)
    .bind(library_only)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_notification_preferences(user_id,kind,enabled)
         VALUES($1,'chapter_created',false)",
    )
    .bind(opted_out)
    .execute(&pool)
    .await
    .unwrap();

    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'Released free chapter','public','free',0,1) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let body = "Nonpaid sample without notification payload body";
    let digest: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(body)
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events
           (chapter_id,version_id,creator_user_id) VALUES($1,$2,$3)",
    )
    .bind(chapter)
    .bind(version)
    .bind(author)
    .execute(&pool)
    .await
    .unwrap();
    let root: String = sqlx::query_scalar(
        "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,
                                  aggregate_id,dedupe_key,payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
                jsonb_build_object('chapter_id',$1,'book_id',$3,'version_id',$4),
                ARRAY['chapter_release_fanout_v1']::text[])
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("ci:s058:chapter-release:{chapter}:{nonce}"))
    .bind(book)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    // Following after the source release cannot retroactively subscribe.
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(late_follower)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    let repo = PgNotifications::new(pool.clone());
    let (left, right) = tokio::join!(
        repo.fanout_chapter_releases(8),
        repo.fanout_chapter_releases(8),
    );
    left.unwrap();
    right.unwrap();
    let child_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='reader.chapter.released' AND aggregate_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(child_count, 3, "follows and library dedupe by reader");
    assert_eq!(repo.fanout_chapter_releases(8).await.unwrap(), 0);
    assert_eq!(repo.deliver_pending(8).await.unwrap(), 3);
    assert_eq!(repo.deliver_pending(8).await.unwrap(), 0);

    let received: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT recipient_user_id,kind,url FROM user_notifications
         WHERE kind='chapter_created' AND recipient_user_id = ANY($1)
         ORDER BY recipient_user_id",
    )
    .bind(&users[1..])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        received,
        vec![
            (
                follower,
                "chapter_created".to_owned(),
                format!("/reader/{chapter}")
            ),
            (
                library_only,
                "chapter_created".to_owned(),
                format!("/reader/{chapter}")
            ),
        ]
    );
    let all_delivered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE (id=$1::uuid OR
                (event_type='reader.chapter.released' AND aggregate_id=$2))
           AND delivered_at IS NOT NULL",
    )
    .bind(&root)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        all_delivered, 4,
        "root and all three children must have receipts"
    );
    let receipt: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='chapter_release_fanout_v1'",
    )
    .bind(&root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt, 1);

    // A book may become age-restricted after its first release. E-08 must
    // re-check recipient eligibility, rather than trust the old fanout.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    for (suffix, permitted) in [("blocked", false), ("consenting", true)] {
        if permitted {
            sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
                .bind(follower)
                .execute(&pool)
                .await
                .unwrap();
        }
        let child: String = sqlx::query_scalar(
            "INSERT INTO outbox_events(
                event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers)
             VALUES('reader.chapter.released',1,'chapter',$1,$2,
                    jsonb_build_object('actor_user_id',$3,
                                       'recipient_user_id',$4,
                                       'object_id',$1,'book_id',$5,
                                       'version_id',$6),
                    ARRAY['user_notifications']::text[])
             RETURNING id::text",
        )
        .bind(chapter)
        .bind(format!("ci:s058:age:{suffix}:{chapter}:{nonce}"))
        .bind(author)
        .bind(follower)
        .bind(book)
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(repo.deliver_pending(8).await.unwrap(), 1);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_notifications
             WHERE recipient_user_id=$1 AND kind='chapter_created'",
        )
        .bind(follower)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            count,
            if permitted { 2 } else { 1 },
            "age-restricted chapter metadata is delivered only to eligible readers"
        );
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event_deliveries
             WHERE event_id=$1::uuid AND consumer='user_notifications'",
        )
        .bind(&child)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            receipt_count, 1,
            "denied events still get a durable receipt"
        );
    }
    s015_notification_privacy_pg::verify(&pool, &repo, &users, book, chapter).await;
    pool.close().await;
}

#[path = "support/s015_notification_privacy_pg.rs"]
mod s015_notification_privacy_pg;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_501_recipients_resume_without_duplicates_and_respect_leases() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never access production");
    let mut lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut lock)
        .await
        .unwrap();
    // Fixture IDs use a disposable high range rather than setval(), which
    // the least-privileged tor_app role is correctly forbidden to call.
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         VALUES((SELECT GREATEST(COALESCE(MAX(id),0),1000000000000::bigint)+1 FROM users),
                $1,'G08 chapter author','$argon2id$test') RETURNING id",
    )
    .bind(format!("g08-author-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru',$2,'public',$3,'basic') RETURNING id",
    )
    .bind(format!("ci:g08:fanout:{nonce}"))
    .bind(format!("G08 release {nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();

    // A single SQL set avoids 501 sequential app-side creates and makes the
    // >2-page acceptance fast enough for the normal PostgreSQL CI lane.
    let mut readers: Vec<i64> = sqlx::query_scalar(
        "WITH id_base AS (
           SELECT GREATEST(COALESCE(MAX(id),0),1000000000000::bigint) AS base
           FROM users
         )
         INSERT INTO users(id,email_normalized,display_name,password_hash)
         SELECT id_base.base + gs.n,
                $1::text || '-' || gs.n::text || '@example.invalid',
                'G08 reader ' || gs.n::text,'$argon2id$test'
         FROM id_base CROSS JOIN generate_series(1,501) AS gs(n)
         RETURNING id",
    )
    .bind(format!("ci-g08-{nonce}"))
    .fetch_all(&pool)
    .await
    .unwrap();
    readers.sort_unstable();
    assert_eq!(readers.len(), 501);
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id)
         SELECT f.user_id,$2 FROM unnest($1::bigint[]) AS f(user_id)",
    )
    .bind(&readers)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    // The root stages eligible IDs, but E-08 must still honor opt-out at
    // delivery time; this reader gets a receipt but no notification.
    sqlx::query(
        "INSERT INTO user_notification_preferences(user_id,kind,enabled)
         VALUES($1,'chapter_created',false)",
    )
    .bind(readers[0])
    .execute(&pool)
    .await
    .unwrap();
    // One reader with both follow and library membership must not be doubled.
    sqlx::query("INSERT INTO reader_library_items(user_id,book_id) VALUES($1,$2)")
        .bind(readers[1])
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'G08 published chapter','public','free',0,1) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(b"G08 metadata only").into();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,'G08 metadata only',$2) RETURNING id",
    )
    .bind(chapter)
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id,creator_user_id)
         VALUES($1,$2,$3)",
    )
    .bind(chapter)
    .bind(version)
    .bind(author)
    .execute(&pool)
    .await
    .unwrap();
    let root: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
                jsonb_build_object('chapter_id',$1,'book_id',$3,'version_id',$4,
                                   'first_release',true),
                ARRAY['chapter_release_fanout_v1']::text[])
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("ci:g08:chapter-published:{chapter}:{version}"))
    .bind(book)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    let late: i64 = sqlx::query_scalar(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         VALUES((SELECT GREATEST(COALESCE(MAX(id),0),1000000000000::bigint)+1 FROM users),
                $1,'Late reader','$argon2id$test') RETURNING id",
    )
    .bind(format!("ci-g08-late-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(late)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    let repo = PgNotifications::new(pool.clone());
    // An event leased by a G-07 worker must remain untouched by the
    // specialized S058 worker, even though the SQL row is not locked.
    sqlx::query(
        "UPDATE outbox_events
         SET lease_epoch=lease_epoch+1,
             lease_until=clock_timestamp()+interval '5 minutes'
         WHERE id=$1::uuid",
    )
    .bind(&root)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(repo.fanout_chapter_releases(1).await.unwrap(), 0);
    let staged_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_release_notification_fanout
         WHERE source_event_id=$1::uuid",
    )
    .bind(&root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(staged_before, 0);
    sqlx::query(
        "UPDATE outbox_events SET lease_until=clock_timestamp()-interval '1 second'
         WHERE id=$1::uuid",
    )
    .bind(&root)
    .execute(&pool)
    .await
    .unwrap();

    // Simulate a worker stopping mid-transaction; SKIP LOCKED must not
    // advance the cursor or acknowledge a root until the lock is released.
    let mut interrupted = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM outbox_events WHERE id=$1::uuid FOR UPDATE")
        .bind(&root)
        .fetch_one(&mut *interrupted)
        .await
        .unwrap();
    assert_eq!(repo.fanout_chapter_releases(1).await.unwrap(), 0);
    interrupted.rollback().await.unwrap();

    assert_eq!(repo.fanout_chapter_releases(1).await.unwrap(), 1);
    let first_cursor: i64 = sqlx::query_scalar(
        "SELECT last_user_id FROM chapter_release_notification_fanout
         WHERE source_event_id=$1::uuid",
    )
    .bind(&root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first_cursor, readers[249]);
    assert_eq!(g08_count_children(&pool, chapter).await, 250);
    // A new instance must resume from the durable cursor, not start at 1.
    drop(repo);
    let restarted = PgNotifications::new(pool.clone());
    assert_eq!(restarted.fanout_chapter_releases(1).await.unwrap(), 1);
    assert_eq!(g08_count_children(&pool, chapter).await, 500);
    assert_eq!(restarted.fanout_chapter_releases(1).await.unwrap(), 1);
    assert_eq!(g08_count_children(&pool, chapter).await, 501);
    let last_cursor: i64 = sqlx::query_scalar(
        "SELECT last_user_id FROM chapter_release_notification_fanout
         WHERE source_event_id=$1::uuid",
    )
    .bind(&root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(last_cursor, readers[500]);
    assert_eq!(restarted.fanout_chapter_releases(1).await.unwrap(), 0);
    let root_receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='chapter_release_fanout_v1'",
    )
    .bind(&root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(root_receipts, 1);
    let root_done: bool =
        sqlx::query_scalar("SELECT delivered_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid")
            .bind(&root)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(root_done);

    // An independently leased child must not be projected prematurely.
    let fenced_child: String = sqlx::query_scalar(
        "SELECT id::text FROM outbox_events
         WHERE event_type='reader.chapter.released' AND aggregate_id=$1
           AND payload->>'recipient_user_id'=$2::text",
    )
    .bind(chapter)
    .bind(readers[2])
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE outbox_events SET lease_epoch=lease_epoch+1,
         lease_until=clock_timestamp()+interval '5 minutes'
         WHERE id=$1::uuid",
    )
    .bind(&fenced_child)
    .execute(&pool)
    .await
    .unwrap();
    for _ in 0..12 {
        if restarted.deliver_pending(100).await.unwrap() == 0 {
            break;
        }
    }
    let receipts_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries d
         JOIN outbox_events e ON e.id=d.event_id
         WHERE e.event_type='reader.chapter.released'
           AND e.aggregate_id=$1 AND d.consumer='user_notifications'",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts_before, 500);
    sqlx::query("UPDATE outbox_events SET lease_until=NULL WHERE id=$1::uuid")
        .bind(&fenced_child)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(restarted.deliver_pending(1).await.unwrap(), 1);
    assert_eq!(restarted.deliver_pending(1).await.unwrap(), 0);
    let receipts_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries d
         JOIN outbox_events e ON e.id=d.event_id
         WHERE e.event_type='reader.chapter.released'
           AND e.aggregate_id=$1 AND d.consumer='user_notifications'",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts_after, 501);

    let distinct_notifications: (i64, i64) = sqlx::query_as(
        "SELECT count(*),count(DISTINCT recipient_user_id)
         FROM user_notifications
         WHERE kind='chapter_created' AND url=$1
           AND recipient_user_id=ANY($2::bigint[])",
    )
    .bind(format!("/reader/{chapter}"))
    .bind(&readers)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        distinct_notifications,
        (500, 500),
        "all eligible readers must get exactly one notification; opt-out gets zero"
    );
    let late_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications
         WHERE recipient_user_id=$1 AND kind='chapter_created' AND url=$2",
    )
    .bind(late)
    .bind(format!("/reader/{chapter}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(late_notifications, 0);
    pool.close().await;
}

async fn g08_count_children(pool: &PgPool, chapter: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='reader.chapter.released' AND aggregate_id=$1",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap()
}
