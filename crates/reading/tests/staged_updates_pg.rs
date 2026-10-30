//! G-08/C-16: on-site V2 revision alerts from durable intent metadata.
//! Isolated PostgreSQL 18 only; never any email, push or paid-body delivery.
use sqlx::PgPool;
use tor_reading::repositories::{PgStagedRevisions, StagedRevisionError};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn staged_v2_is_private_and_disappears_when_old_hidden_or_unfollowed() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18 fixture");
    assert!(url.ends_with("/tor_ci"), "never modify a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp())*1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut users = Vec::new();
    for label in ["author", "follower", "outsider"] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Staged test','$argon2id$fixture') RETURNING id",
        )
        .bind(format!("c-staged-{label}-{nonce}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(id);
    }
    let (author, follower, outsider) = (users[0], users[1], users[2]);
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Reader staged book','public',$2) RETURNING id",
    )
    .bind(format!("ci:c-staged:{nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()-interval '2 days')",
    )
    .bind(follower)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'New V2 revision','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Private-notification-proof-v1') RETURNING id",
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
        "INSERT INTO chapter_first_release_events(chapter_id,version_id) VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .unwrap();
    let v1_event: String = sqlx::query_scalar(
        "INSERT INTO outbox_events(
            event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
            payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
            jsonb_build_object('chapter_id',$1,'book_id',$3,
                               'version_id',$4,'published_at',now(),
                               'first_release',true),
            ARRAY['subscription-inbox-v1']::text[]) RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("staged-first:{nonce}"))
    .bind(book)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Private-notification-proof-v2') RETURNING id",
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
    let v2_event: String = sqlx::query_scalar(
        "INSERT INTO outbox_events(
            event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
            payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
            jsonb_build_object('chapter_id',$1,'book_id',$3,
                               'version_id',$4,'published_at',now(),
                               'first_release',false),
            ARRAY['subscription-inbox-v1','search-index-v1']::text[])
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("staged-revision:{nonce}"))
    .bind(book)
    .bind(v2)
    .fetch_one(&pool)
    .await
    .unwrap();

    for event in [&v1_event, &v2_event] {
        sqlx::query(
            "INSERT INTO notification_event_recipient_intents(event_id,user_id)
             VALUES($1::uuid,$2) ON CONFLICT DO NOTHING",
        )
        .bind(event)
        .bind(follower)
        .execute(&pool)
        .await
        .unwrap();
    }
    let revisions = PgStagedRevisions::new(pool.clone());
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty(),
        "uncommitted batches must not appear on the reader page");

    sqlx::query(
        "INSERT INTO notification_event_fanout_scans(event_id,completed_at)
         VALUES($1::uuid,now())",
    )
    .bind(&v2_event)
    .execute(&pool)
    .await
    .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty(),
        "a checkpoint without consumer receipt is not completion");
    sqlx::query(
        "INSERT INTO event_deliveries(consumer,event_id)
         VALUES('subscription-inbox-v1',$1::uuid)",
    )
    .bind(&v2_event)
    .execute(&pool)
    .await
    .unwrap();
    let page = revisions.latest(follower, None).await.unwrap();
    assert_eq!(page.items.len(), 1, "V1 must not duplicate the C-16 feed");
    assert_eq!(page.items[0].chapter_id, chapter);
    assert_eq!(page.items[0].version_no, 2);
    assert_eq!(page.next_cursor, None);
    assert!(!page.items[0].is_paid);
    assert_eq!(page.items[0].book_title, "Reader staged book");
    assert!(!format!("{page:?}").contains("Private-notification-proof"));
    assert!(revisions.latest(outsider, None).await.unwrap().items.is_empty());
    assert_eq!(revisions.latest(0, None).await, Err(StagedRevisionError::Invalid));

    // No actor can read another user's intent merely by knowing its event UUID.
    sqlx::query(
        "INSERT INTO notification_event_recipient_intents(event_id,user_id)
         VALUES($1::uuid,$2)",
    )
    .bind(&v2_event)
    .bind(outsider)
    .execute(&pool)
    .await
    .unwrap();
    assert!(revisions.latest(outsider, None).await.unwrap().items.is_empty());

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty());
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
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty());
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET access_level='paid',price_amount=100 WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty(),
        "paid chapter without protected storage is not advertised");
    sqlx::query("UPDATE chapters SET access_level='free',price_amount=0 WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    let v3: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,3,'No V2 link after re-point') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v3)
        .execute(&pool)
        .await
        .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty(),
        "a later revision must hide the stale V2 notification");
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(revisions.latest(follower, None).await.unwrap().items.len(), 1);
    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=$1 AND book_id=$2")
        .bind(follower)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty());
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()+interval '1 day')",
    )
    .bind(follower)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert!(revisions.latest(follower, None).await.unwrap().items.is_empty(),
        "re-follow never retroactively shows older V2 intents");
    pool.close().await;
}
