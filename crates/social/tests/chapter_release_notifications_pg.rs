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
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$test') RETURNING id",
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
    pool.close().await;
}
