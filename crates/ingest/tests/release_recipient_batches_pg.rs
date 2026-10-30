//! G-08: 201+ explicit followers, opt-out, first release and paid metadata.
//! Runs exclusively against disposable PostgreSQL 18 /tor_ci.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use std::collections::HashSet;
use tor_ingest::repositories::release_recipients::{
    PgReleaseRecipients, RecipientError, RECIPIENT_BATCH_SIZE,
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn g_08_201_followers_paged_without_duplicates_or_101st_loss() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated database URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'G08 Author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("g08-author-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','G08 Book','public',$2) RETURNING id",
    )
    .bind(format!("ci:g08:book:{nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut followers: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         SELECT $1 || '-' || n::text || '@example.invalid',
                'G08 Reader','$argon2id$fixture'
         FROM generate_series(1,201) AS n RETURNING id",
    )
    .bind(format!("g08-reader-{nonce}"))
    .fetch_all(&pool)
    .await
    .unwrap();
    followers.sort_unstable();
    assert_eq!(followers.len(), 201);
    assert_eq!(followers.iter().copied().collect::<HashSet<_>>().len(), 201);
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         SELECT recipient.user_id,$1,now()-interval '1 day'
         FROM unnest($2::bigint[]) AS recipient(user_id)",
    )
    .bind(book)
    .bind(&followers[..])
    .execute(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Бесплатная глава','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Текст бесплатной главы') RETURNING id",
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
        "INSERT INTO chapter_first_release_events(chapter_id,version_id,created_at)
         VALUES($1,$2,now()-interval '1 hour')",
    )
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();

    let selector = PgReleaseRecipients::new(pool.clone());
    assert_eq!(RECIPIENT_BATCH_SIZE, 100);
    assert_eq!(
        selector.next_batch(0, None).await,
        Err(RecipientError::Invalid)
    );
    assert_eq!(
        selector.next_batch(chapter, Some(-1)).await,
        Err(RecipientError::Invalid)
    );
    assert_eq!(
        selector.still_candidate(chapter, 0).await,
        Err(RecipientError::Invalid)
    );
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    let mut cursor = None;
    for _ in 0..4 {
        let page = selector.next_batch(chapter, cursor).await.unwrap();
        sizes.push(page.items.len());
        for item in page.items {
            assert_eq!(item.book_id, book);
            assert_eq!(item.chapter_id, chapter);
            assert!(!item.is_paid);
            assert_eq!(item.price_minor, 0);
            seen.push(item.user_id);
        }
        match page.next_after_user {
            Some(next) => {
                assert_eq!(seen.last(), Some(&next));
                cursor = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(sizes, vec![100, 100, 1]);
    assert_eq!(seen, followers, "the 101st and 201st are never omitted");
    assert_eq!(seen.iter().copied().collect::<HashSet<_>>().len(), 201);
    assert!(selector
        .next_batch(chapter, Some(followers[200]))
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        selector.next_batch(chapter, None).await.unwrap().items[0].user_id,
        followers[0]
    );

    // A read marker is not an unsubscribe or a delivery receipt.
    sqlx::query("INSERT INTO reader_follow_notification_reads(user_id,chapter_id) VALUES($1,$2)")
        .bind(followers[0])
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(selector
        .still_candidate(chapter, followers[0])
        .await
        .unwrap());
    let late: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Late Reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("g08-late-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(late)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!selector.still_candidate(chapter, late).await.unwrap());
    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=$1 AND book_id=$2")
        .bind(followers[100])
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!selector
        .still_candidate(chapter, followers[100])
        .await
        .unwrap());
    assert!(selector
        .still_candidate(chapter, followers[200])
        .await
        .unwrap());
    let suffix = selector
        .next_batch(chapter, Some(followers[99]))
        .await
        .unwrap();
    assert_eq!(suffix.items.len(), 100);
    assert_eq!(suffix.items.last().unwrap().user_id, followers[200]);

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(selector
        .next_batch(chapter, None)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!selector
        .still_candidate(chapter, followers[0])
        .await
        .unwrap());
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET title='Исправленный заголовок' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    let release_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(release_count, 1);
    assert_eq!(
        selector
            .next_batch(chapter, None)
            .await
            .unwrap()
            .items
            .len(),
        100
    );

    // Paid chapter is invisible before verified encrypted storage is attached.
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Платная глава','public','paid',19900) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Платный текст') RETURNING id",
    )
    .bind(paid)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(paid)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chapter_first_release_events(chapter_id,version_id) VALUES($1,$2)")
        .bind(paid)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    assert!(selector
        .next_batch(paid, None)
        .await
        .unwrap()
        .items
        .is_empty());
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
           decode(repeat('11',32),'hex'),sha256(convert_to('Платный текст','UTF8')),
           decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),
           convert_to('tor.encrypted.chapter.v1' || E'\\n' || $2::text || E'\\n' ||
              $3::text || E'\\n' || $1::text || E'\\n' ||
              encode(sha256(convert_to('Платный текст','UTF8')),'hex') || E'\\n','UTF8'),
           'gzip',now(),now())",
    )
    .bind(&object_id)
    .bind(paid)
    .bind(paid_version)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::text::uuid WHERE id=$1")
        .bind(paid_version)
        .bind(&object_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_encrypted_release_events
         (chapter_id,version_id,storage_object_id) VALUES($1,$2,$3::text::uuid)",
    )
    .bind(paid)
    .bind(paid_version)
    .bind(&object_id)
    .execute(&pool)
    .await
    .unwrap();
    let paid_page = selector.next_batch(paid, None).await.unwrap();
    assert_eq!(paid_page.items.len(), 100);
    assert!(paid_page
        .items
        .iter()
        .all(|r| r.is_paid && r.price_minor == 19900));
    assert!(selector
        .still_candidate(paid, followers[200])
        .await
        .unwrap());
    // Following after the free release must still include a NEW paid release.
    assert!(selector.still_candidate(paid, late).await.unwrap());

    // CI destroys its disposable database. Never delete permanent book records.
    pool.close().await;
}
