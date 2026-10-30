//! G-08: isolated PostgreSQL evidence for 201+ bounded notification candidates.
//! This selector never sends external messages or writes recipient receipts.
use sqlx::PgPool;
use tor_ingest::repositories::notify::PgNotificationRecipients;
use tor_ingest::services::notify::RecipientError;

async fn fixture(pool: &PgPool, label: &str) -> (i64, i64, i64) {
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'G08 author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("g08-author-{label}@example.invalid"))
    .fetch_one(pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','G08 publication','public',$2) RETURNING id",
    )
    .bind(format!("ci:g08:{label}"))
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'G08 first chapter','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'No private content in notification') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id,created_at)
         VALUES($1,$2,now()-interval '2 days')",
    )
    .bind(chapter)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    (book, chapter, version)
}

async fn published_event(
    pool: &PgPool,
    label: &str,
    book: i64,
    chapter: i64,
    version: i64,
    age_days: i32,
    first: bool,
) -> String {
    sqlx::query_scalar(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
                jsonb_build_object(
                    'chapter_id',$1,'book_id',$3,'version_id',$4,
                    'published_at',now()-($5::integer * interval '1 day'),
                    'first_release',$6),
                ARRAY['search-index-v1','subscription-inbox-v1']::text[])
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("g08-published:{label}:{version}"))
    .bind(book)
    .bind(version)
    .bind(age_days)
    .bind(first)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "only disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_201_subscribers_are_paged_100_100_1_without_duplicates() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG18");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let label = format!("batch-{nonce}");
    let (book, chapter, version) = fixture(&pool, &label).await;
    let prefix = format!("g08-reader-{nonce}-");
    sqlx::query(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         SELECT $1 || n::text || '@example.invalid',
                'G08 candidate','$argon2id$fixture'
         FROM generate_series(1,201) AS recipients(n)",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         SELECT id,$1,now()-interval '3 days'
         FROM users WHERE email_normalized LIKE $2",
    )
    .bind(book)
    .bind(format!("{prefix}%"))
    .execute(&pool)
    .await
    .unwrap();
    let event_id = published_event(&pool, &label, book, chapter, version, 0, true).await;
    // This reader explicitly follows only AFTER the immutable publication.
    let late: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Late follow','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("g08-late-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()+interval '1 day')",
    )
    .bind(late)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let reader = PgNotificationRecipients::new(pool.clone());
    let one = reader.next_page(&event_id, None).await.unwrap();
    assert_eq!(one.user_ids.len(), 100);
    assert_eq!(one.next_user_id, one.user_ids.last().copied());
    let two = reader.next_page(&event_id, one.next_user_id).await.unwrap();
    assert_eq!(two.user_ids.len(), 100);
    let three = reader.next_page(&event_id, two.next_user_id).await.unwrap();
    assert_eq!(three.user_ids.len(), 1);
    assert_eq!(three.next_user_id, None);
    let all = [one.user_ids.clone(), two.user_ids, three.user_ids].concat();
    assert_eq!(all.len(), 201, "recipient 201 must not disappear");
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!all.contains(&late));
    assert_eq!(
        reader.next_page(&event_id, None).await.unwrap().user_ids,
        one.user_ids,
        "replaying a page cannot mutate or skip recipient candidates"
    );
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        receipt_count, 0,
        "candidate listing does not claim delivery"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_privacy_visibility_unfollow_and_v2_follow_window() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG18");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let label = format!("visibility-{nonce}");
    let (book, chapter, v1) = fixture(&pool, &label).await;
    let mut readers = Vec::new();
    for index in 1..=3 {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'G08 reader','$argon2id$fixture') RETURNING id",
        )
        .bind(format!("g08-visibility-{nonce}-{index}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
        readers.push(user);
    }
    // One follow before V1, another between V1 and V2, one after V2.
    for (user_id, interval_days) in [(readers[0], 3), (readers[1], 1), (readers[2], -1)] {
        sqlx::query(
            "INSERT INTO reader_book_follows(user_id,book_id,created_at)
             VALUES($1,$2,now()-($3::integer * interval '1 day'))",
        )
        .bind(user_id)
        .bind(book)
        .bind(interval_days)
        .execute(&pool)
        .await
        .unwrap();
    }
    let event_v1 = published_event(&pool, &label, book, chapter, v1, 2, true).await;
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Updated free chapter') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE chapter_publications SET version_id=$2,published_at=now()
         WHERE chapter_id=$1",
    )
    .bind(chapter)
    .bind(v2)
    .execute(&pool)
    .await
    .unwrap();
    let event_v2 = published_event(&pool, &label, book, chapter, v2, 0, false).await;
    let recipient = PgNotificationRecipients::new(pool.clone());
    assert!(
        recipient
            .next_page(&event_v1, None)
            .await
            .unwrap()
            .user_ids
            .is_empty(),
        "superseded chapter version must not cause stale notifications"
    );
    let page = recipient.next_page(&event_v2, None).await.unwrap();
    assert_eq!(page.user_ids, vec![readers[0], readers[1]]);
    assert!(!page.user_ids.contains(&readers[2]));

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(recipient
        .next_page(&event_v2, None)
        .await
        .unwrap()
        .user_ids
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(recipient
        .next_page(&event_v2, None)
        .await
        .unwrap()
        .user_ids
        .is_empty());
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE chapters SET reader_released_at=now()+interval '1 day'
         WHERE id=$1",
    )
    .bind(chapter)
    .execute(&pool)
    .await
    .unwrap();
    assert!(recipient
        .next_page(&event_v2, None)
        .await
        .unwrap()
        .user_ids
        .is_empty());
    sqlx::query("UPDATE chapters SET reader_released_at=NULL WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET access_level='paid',price_amount=100 WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        recipient
            .next_page(&event_v2, None)
            .await
            .unwrap()
            .user_ids
            .is_empty(),
        "unverified paid content must never be advertised"
    );
    sqlx::query("UPDATE chapters SET access_level='free',price_amount=0 WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=$1 AND book_id=$2")
        .bind(readers[0])
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        recipient.next_page(&event_v2, None).await.unwrap().user_ids,
        vec![readers[1]],
        "unfollow removes user from future candidate batches"
    );
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()+interval '1 day')",
    )
    .bind(readers[0])
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        recipient.next_page(&event_v2, None).await.unwrap().user_ids,
        vec![readers[1]],
        "re-follow cannot retroactively subscribe to old publications"
    );
    assert_eq!(
        recipient.next_page("bad", None).await,
        Err(RecipientError::Invalid)
    );
    assert_eq!(
        recipient
            .next_page("00000000-0000-0000-0000-000000000000", None)
            .await,
        Err(RecipientError::Invalid)
    );
    assert_eq!(
        recipient.next_page(&event_v2, Some(0)).await,
        Err(RecipientError::Invalid)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_fenced_staging_201_intents_is_atomic_and_resumable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let label = format!("staging-{nonce}");
    let (book, chapter, version) = fixture(&pool, &label).await;
    let prefix = format!("g08-staging-{nonce}-");
    sqlx::query(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         SELECT $1 || n::text || '@example.invalid',
                'G08 staging reader','$argon2id$fixture'
         FROM generate_series(1,201) AS recipients(n)",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         SELECT id,$1,now()-interval '3 days'
         FROM users WHERE email_normalized LIKE $2",
    )
    .bind(book)
    .bind(format!("{prefix}%"))
    .execute(&pool)
    .await
    .unwrap();
    let event_id = published_event(&pool, &label, book, chapter, version, 0, true).await;
    sqlx::query(
        "UPDATE outbox_events
         SET lease_epoch=1,lease_until=now()+interval '60 seconds'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();

    let worker = PgNotificationRecipients::new(pool.clone());
    let one = worker.stage_next(&event_id, 1).await.unwrap();
    assert_eq!(one.newly_staged, 100);
    assert!(!one.complete);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_event_recipient_intents
         WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 100, "page and checkpoint committed together");
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='subscription-inbox-v1'",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipts, 0,
        "partial page cannot acknowledge whole consumer"
    );

    // Simulate a crash after a committed batch, expiry and legal reclaim.
    // The new process must resume from the stored cursor with a new epoch.
    sqlx::query(
        "UPDATE outbox_events SET lease_until=now()-interval '1 second'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let reclaimed_epoch: i64 = sqlx::query_scalar(
        "UPDATE outbox_events
         SET lease_epoch=lease_epoch+1,lease_until=now()+interval '60 seconds'
         WHERE id=$1::uuid AND lease_until<now()
         RETURNING lease_epoch",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reclaimed_epoch, 2);
    assert_eq!(
        worker.stage_next(&event_id, 1).await,
        Err(RecipientError::LostLease)
    );
    let restarted = PgNotificationRecipients::new(pool.clone());
    let two = restarted
        .stage_next(&event_id, reclaimed_epoch)
        .await
        .unwrap();
    assert_eq!(two.newly_staged, 100);
    assert!(!two.complete);
    assert!(two.cursor_user_id > one.cursor_user_id);
    sqlx::query(
        "INSERT INTO event_deliveries(consumer,event_id)
         VALUES('search-index-v1',$1::uuid)",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let three = restarted
        .stage_next(&event_id, reclaimed_epoch)
        .await
        .unwrap();
    assert_eq!(three.newly_staged, 1);
    assert!(three.complete);
    assert!(three.cursor_user_id > two.cursor_user_id);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM notification_event_recipient_intents
         WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 201);
    let final_state: (bool, bool, i64) = sqlx::query_as(
        "SELECT delivered_at IS NOT NULL,lease_until IS NULL,
                (SELECT COUNT(*) FROM event_deliveries d WHERE d.event_id=e.id)
         FROM outbox_events e WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(final_state, (true, true, 2));
    assert_eq!(
        restarted.stage_next(&event_id, reclaimed_epoch).await,
        Err(RecipientError::LostLease)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_staging_rejects_superseded_and_expired_leases_without_writes() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let label = format!("fence-{nonce}");
    let (book, chapter, version) = fixture(&pool, &label).await;
    let event_id = published_event(&pool, &label, book, chapter, version, 0, true).await;
    sqlx::query(
        "UPDATE outbox_events
         SET lease_epoch=2,lease_until=now()+interval '60 seconds'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let worker = PgNotificationRecipients::new(pool.clone());
    assert_eq!(
        worker.stage_next(&event_id, 1).await,
        Err(RecipientError::LostLease)
    );
    assert_eq!(
        worker.stage_next(&event_id, 0).await,
        Err(RecipientError::Invalid)
    );
    sqlx::query("UPDATE outbox_events SET lease_until=now()-interval '1 second' WHERE id=$1::uuid")
        .bind(&event_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        worker.stage_next(&event_id, 2).await,
        Err(RecipientError::LostLease)
    );
    let checkpoints: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_event_fanout_scans WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        checkpoints, 0,
        "a stale worker cannot advance durable progress"
    );

    sqlx::query(
        "UPDATE outbox_events SET lease_epoch=3,lease_until=now()+interval '60 seconds'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let completed = worker.stage_next(&event_id, 3).await.unwrap();
    assert!(
        completed.complete,
        "no eligible followers is a complete scan"
    );
    assert_eq!(completed.newly_staged, 0);
    let delivered: Option<String> =
        sqlx::query_scalar("SELECT delivered_at::text FROM outbox_events WHERE id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        delivered.is_none(),
        "independent search consumer is still pending"
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='subscription-inbox-v1'",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipts, 1,
        "only the staged internal consumer is acknowledged"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn g_08_paid_publication_requires_verified_encrypted_object() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG18");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let label = format!("paid-verified-{nonce}");
    let (book, _, _) = fixture(&pool, &label).await;
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Paid follower','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("g08-paid-reader-{label}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()-interval '1 day')",
    )
    .bind(reader)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Новая платная глава','public','paid',19900) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Платный текст') RETURNING id",
    )
    .bind(chapter)
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
        "INSERT INTO chapter_first_release_events(chapter_id,version_id)
         VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();
    let event_id = published_event(&pool, &label, book, chapter, version, 0, true).await;
    let recipients = PgNotificationRecipients::new(pool.clone());
    assert!(
        recipients.next_page(&event_id, None).await.unwrap().user_ids.is_empty(),
        "unverified paid chapter cannot be offered in notification delivery"
    );
    let object_id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects(
           id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
           kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES($1::text::uuid,'chapter','tor-private-chapter-ci',
           'private/chapter-versions/' || $1::text || '.enc','attached',64,
           decode(repeat('11',32),'hex'),
           sha256(convert_to('Платный текст','UTF8')),
           decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),
           convert_to('tor.encrypted.chapter.v1' || E'\n' || $2::text || E'\n' ||
             $3::text || E'\n' || $1::text || E'\n' ||
             encode(sha256(convert_to('Платный текст','UTF8')),'hex') || E'\n','UTF8'),
           'gzip',now(),now())",
    )
    .bind(&object_id)
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::text::uuid WHERE id=$1")
        .bind(version)
        .bind(&object_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_encrypted_release_events(chapter_id,version_id,storage_object_id)
         VALUES($1,$2,$3::text::uuid)",
    )
    .bind(chapter)
    .bind(version)
    .bind(&object_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        recipients.next_page(&event_id, None).await.unwrap().user_ids,
        vec![reader],
        "verified paid chapter is an eligible release, without granting access"
    );
    sqlx::query(
        "UPDATE outbox_events SET lease_epoch=1,lease_until=now()+interval '60 seconds'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let staged = recipients.stage_next(&event_id, 1).await.unwrap();
    assert!(staged.complete);
    assert_eq!(staged.newly_staged, 1);
    let receipts: Vec<String> = sqlx::query_scalar(
        "SELECT consumer FROM event_deliveries WHERE event_id=$1::uuid ORDER BY consumer",
    )
    .bind(&event_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, vec!["subscription-inbox-v1"]);
    let delivered: Option<String> =
        sqlx::query_scalar("SELECT delivered_at::text FROM outbox_events WHERE id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(delivered.is_none(), "search consumer has not completed");
    pool.close().await;
}
