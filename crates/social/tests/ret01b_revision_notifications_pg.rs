//! RET-01b: disposable PostgreSQL V2 root -> version-scoped reader bell.
use sqlx::PgPool;
use tor_social::repositories::notifications::PgNotifications;
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn ret01b_free_revision_fanout_is_version_aware_and_not_retroactive() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "production forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for suffix in ["owner", "reader", "late"] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'RET revision','$argon2id$test') RETURNING id",
        )
        .bind(format!("ret01b-{suffix}-{nonce}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(id);
    }
    let (owner, reader, late) = (users[0], users[1], users[2]);
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','RET V2 book','public',$2,'basic') RETURNING id",
    )
    .bind(format!("ci:ret01b:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'RET V2 chapter','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'RET old') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id,creator_user_id)
         VALUES($1,$2,$3)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(reader)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'RET revised') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    let root = queue_revision_root(&pool, book, chapter, v2).await;
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(late)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    let notifications = PgNotifications::new(pool.clone());
    assert_eq!(notifications.fanout_chapter_releases(1).await.unwrap(), 1);
    assert_eq!(notifications.fanout_chapter_releases(1).await.unwrap(), 0);
    let payloads: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT (payload->>'recipient_user_id')::bigint,
                (payload->>'version_id')::bigint,
                event_type
         FROM outbox_events
         WHERE event_type='reader.chapter.updated' AND aggregate_id=$1",
    )
    .bind(chapter)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        payloads,
        vec![(reader, v2, "reader.chapter.updated".into())]
    );
    let complete: bool =
        sqlx::query_scalar("SELECT delivered_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid")
            .bind(&root)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        complete,
        "the revision root is acknowledged only after fanout"
    );
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 1);
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 0);
    let private_bell: (String, String) = sqlx::query_as(
        "SELECT kind,url FROM user_notifications
         WHERE recipient_user_id=$1 AND kind='chapter_updated'",
    )
    .bind(reader)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        private_bell,
        ("chapter_updated".into(), format!("/read/{chapter}"))
    );
    let too_late: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications
         WHERE recipient_user_id=$1 AND kind='chapter_updated'",
    )
    .bind(late)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(too_late, 0);
    let unexpected_first: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications
         WHERE recipient_user_id=$1 AND kind='chapter_created'",
    )
    .bind(reader)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unexpected_first, 0, "V2 must not masquerade as V1");
    let releases = notifications
        .list_section(reader, None, 10, "releases")
        .await
        .unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].kind, "chapter_updated");
    assert!(notifications
        .list_section(reader, None, 10, "system")
        .await
        .unwrap()
        .is_empty());

    // A queued or already projected V2 is never a perpetual visibility grant.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(notifications
        .list(reader, None, 10)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(notifications.list(reader, None, 10).await.unwrap().len(), 1);
    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=$1 AND book_id=$2")
        .bind(reader)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        notifications
            .list(reader, None, 10)
            .await
            .unwrap()
            .is_empty(),
        "an unfollow hides the private V2 notice immediately"
    );
    // Only the late follower is now subscribed. Fan out V3 while it is
    // current, then move the publication to V4 before the delivery worker.
    let v3 = publish_revision_fixture(&pool, chapter, 3, "RET changed again").await;
    let v3_root = queue_revision_root(&pool, book, chapter, v3).await;
    assert_eq!(notifications.fanout_chapter_releases(1).await.unwrap(), 1);
    let stale: String = sqlx::query_scalar(
        "SELECT id::text FROM outbox_events WHERE aggregate_id=$1
         AND event_type='reader.chapter.updated' AND delivered_at IS NULL
         AND payload->>'source_event_id'=$2 AND (payload->>'recipient_user_id')::bigint=$3
         AND (payload->>'version_id')::bigint=$4",
    )
    .bind(chapter)
    .bind(&v3_root)
    .bind(late)
    .bind(v3)
    .fetch_one(&pool)
    .await
    .unwrap();

    let v4 = publish_revision_fixture(&pool, chapter, 4, "RET newest chapter").await;
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 1);
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 0);
    let stale_state: (bool, bool, bool) = sqlx::query_as(
        "SELECT e.delivered_at IS NOT NULL,
                EXISTS(SELECT 1 FROM event_deliveries d
                       WHERE d.event_id=e.id AND d.consumer='user_notifications'),
                EXISTS(SELECT 1 FROM user_notifications n WHERE n.source_event_id=e.id)
         FROM outbox_events e WHERE e.id=$1::uuid",
    )
    .bind(&stale)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stale_state, (true, true, false));

    // The same still-eligible reader does receive an event for current V4.
    let v4_root = queue_revision_root(&pool, book, chapter, v4).await;
    assert_eq!(notifications.fanout_chapter_releases(1).await.unwrap(), 1);
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 1);
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 0);
    let visible: Vec<(i64, String)> = sqlx::query_as(
        "SELECT (e.payload->>'version_id')::bigint,n.url
         FROM user_notifications n JOIN outbox_events e ON e.id=n.source_event_id
         WHERE n.recipient_user_id=$1 AND n.kind='chapter_updated' AND e.aggregate_id=$2
           AND e.payload->>'source_event_id'=$3",
    )
    .bind(late)
    .bind(chapter)
    .bind(&v4_root)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(visible, vec![(v4, format!("/read/{chapter}"))]);
    std::io::Write::write_all(
        &mut std::io::stdout().lock(),
        b"D001_SUPERSEDED_FREE_REVISION_BELL_SUPPRESSED_OK\n",
    )
    .unwrap();
    pool.close().await;
}

async fn publish_revision_fixture(pool: &PgPool, chapter: i64, number: i32, body: &str) -> i64 {
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(number)
    .bind(body)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    version
}

async fn queue_revision_root(pool: &PgPool, book: i64, chapter: i64, version: i64) -> String {
    sqlx::query_scalar(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
                jsonb_build_object('chapter_id',$1,'book_id',$3,'version_id',$4,
                                   'first_release',false,'published_at',clock_timestamp()),
                ARRAY['chapter_revision_fanout_v1']::text[])
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("ci:ret01b:{chapter}:{version}"))
    .bind(book)
    .bind(version)
    .fetch_one(pool)
    .await
    .unwrap()
}
