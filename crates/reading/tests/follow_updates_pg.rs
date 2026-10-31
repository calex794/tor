//! C-16: real PostgreSQL evidence for private, after-follow release updates.
use sqlx::PgPool;
use tor_reading::repositories::{PgFollowUpdates, UpdatesCursor};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn c_16_followed_free_releases_are_private_and_keyset_paginated() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated postgres");
    assert!(url.ends_with("/tor_ci"), "never test production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let author_email = format!("c16-author-{nonce}@example.invalid");
    let reader_email = format!("c16-reader-{nonce}@example.invalid");
    let outsider_email = format!("c16-other-{nonce}@example.invalid");
    let mut actors = Vec::new();
    for email in [author_email, reader_email, outsider_email] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash)
             VALUES ($1,'C16 Test','$argon2id$fixture') RETURNING id",
        )
        .bind(email)
        .fetch_one(&pool)
        .await
        .unwrap();
        actors.push(id);
    }
    let author = actors[0];
    let reader = actors[1];
    let outsider = actors[2];
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books (source_identity,target_language,title,visibility,owner_user_id)
         VALUES ($1,'ru','C16 Book','public',$2) RETURNING id",
    )
    .bind(format!("ci:c16:{nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    let releases = PgFollowUpdates::new(pool.clone());
    assert!(releases
        .latest(reader, None)
        .await
        .unwrap()
        .items
        .is_empty());

    // The first chapter is published BEFORE the reader follows the book.
    let mut chapter_ids = Vec::new();
    for number in 1..=33_i32 {
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters
              (book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES ($1,$2,$3,'public','free',0) RETURNING id",
        )
        .bind(book)
        .bind(number)
        .bind(format!("Глава {number}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown)
             VALUES ($1,1,'Текст главы') RETURNING id",
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chapter_publications (chapter_id,version_id) VALUES ($1,$2)")
            .bind(chapter)
            .bind(version)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chapter_first_release_events (chapter_id,version_id,created_at)
             VALUES ($1,$2,now() - ((34 - $3) * interval '1 second'))",
        )
        .bind(chapter)
        .bind(version)
        .bind(number)
        .execute(&pool)
        .await
        .unwrap();
        chapter_ids.push(chapter);
    }
    // Reference one immutable release timestamp, not now() in a later
    // statement: slow CI can cross the half-second boundary during setup.
    // Chapter 2 is at least one second after chapter 1 in the fixture.
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         SELECT $1,$2,created_at + interval '0.5 seconds'
         FROM chapter_first_release_events WHERE chapter_id=$3",
    )
    .bind(reader)
    .bind(book)
    .bind(chapter_ids[0])
    .execute(&pool)
    .await
    .unwrap();
    let first = releases.latest(reader, None).await.unwrap();
    assert_eq!(first.items.len(), 30);
    assert!(first.next_cursor.is_some());
    assert!(first.items.iter().all(|item| item.book_id == book));
    assert!(!first
        .items
        .iter()
        .any(|item| item.chapter_id == chapter_ids[0]));
    assert!(releases
        .latest(outsider, None)
        .await
        .unwrap()
        .items
        .is_empty());
    let second = releases.latest(reader, first.next_cursor).await.unwrap();
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.next_cursor, None);
    assert!(second.items.iter().all(|item| item.chapter_number >= 2));
    let free = chapter_ids[1];
    assert!(releases.set_read_state(reader, free, true).await.unwrap());
    assert!(releases.set_read_state(reader, free, true).await.unwrap());
    let free_page = releases.latest(reader, first.next_cursor).await.unwrap();
    assert!(free_page
        .items
        .iter()
        .any(|item| item.chapter_id == free && item.is_read));
    assert!(!releases.set_read_state(outsider, free, true).await.unwrap());
    assert!(!releases
        .set_read_state(reader, chapter_ids[0], true)
        .await
        .unwrap());
    assert!(releases.set_read_state(reader, free, false).await.unwrap());
    assert!(releases.set_read_state(reader, free, false).await.unwrap());
    assert!(
        !releases
            .latest(reader, first.next_cursor)
            .await
            .unwrap()
            .items
            .iter()
            .find(|item| item.chapter_id == free)
            .unwrap()
            .is_read
    );

    // A paid chapter is visible as a purchase opportunity only after B/G
    // record a verified encrypted release. Unverified paid publication alone
    // must never advertise a ready chapter.
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,34,'Платная глава','public','paid',19900) RETURNING id",
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
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id)
         VALUES($1,$2)",
    )
    .bind(paid)
    .bind(paid_version)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!releases
        .latest(reader, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|x| x.chapter_id == paid));
    assert!(!releases.set_read_state(reader, paid, true).await.unwrap());

    // Production B publishes only after G verified and attached ciphertext.
    // The isolated fixture records the same immutable B/G proof shape.
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
    sqlx::query(
        "UPDATE chapter_versions SET storage_object_id=$2::text::uuid
         WHERE id=$1",
    )
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
    let verified = releases.latest(reader, None).await.unwrap();
    let offer = verified
        .items
        .iter()
        .find(|x| x.chapter_id == paid)
        .unwrap();
    assert!(
        offer.is_paid,
        "verified paid release belongs in update list"
    );
    assert_eq!(offer.price_minor, 19900);
    assert!(!offer.is_read);
    assert!(releases.set_read_state(reader, paid, true).await.unwrap());
    let confirmed = releases.latest(reader, None).await.unwrap();
    assert!(confirmed
        .items
        .iter()
        .any(|item| item.chapter_id == paid && item.is_paid && item.is_read));
    assert!(releases.set_read_state(reader, paid, false).await.unwrap());
    assert!(releases
        .latest(reader, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|item| item.chapter_id == paid && !item.is_read));

    assert!(!releases
        .latest(outsider, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|x| x.chapter_id == paid));

    // The latest chapter disappears if made private; edits cannot create a
    // second first-release row because chapter_id is the primary key.
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(chapter_ids[32])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(releases.latest(reader, None).await.unwrap().items.len(), 30);
    assert!(!releases
        .set_read_state(reader, chapter_ids[32], true)
        .await
        .unwrap());

    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=$1 AND book_id=$2")
        .bind(reader)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(releases
        .latest(reader, None)
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(!releases.set_read_state(reader, paid, true).await.unwrap());

    assert!(UpdatesCursor::parse("bad-cursor").is_none());
    // The disposable tor_ci database is discarded by the workflow. Do not
    // delete owned books manually across RESTRICT ownership constraints.
    pool.close().await;
}


/// E-05/S014: creator follows extend the existing on-site release feed across
/// books. Historical periods are respected; a book+creator overlap is one row.
async fn insert_e05_free_release(
    pool: &PgPool,
    book_id: i64,
    creator_id: i64,
    number: i32,
    title: &str,
    anchor: &str,
    offset_micros: i64,
) -> i64 {
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters
           (book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES ($1,$2,$3,'public','free',0) RETURNING id",
    )
    .bind(book_id)
    .bind(number)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES ($1,1,'E-05 feed fixture') RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2)")
        .bind(chapter_id)
        .bind(version_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events
           (chapter_id,version_id,creator_user_id,created_at)
         VALUES ($1,$2,$3,$4::timestamptz + ($5::bigint * interval '1 microsecond'))",
    )
    .bind(chapter_id)
    .bind(version_id)
    .bind(creator_id)
    .bind(anchor)
    .bind(offset_micros)
    .execute(pool)
    .await
    .unwrap();
    chapter_id
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn e_05_author_followed_releases_are_cross_book_period_scoped_and_deduplicated() {
    use std::collections::HashSet;

    let url = std::env::var("TEST_DATABASE_URL").expect("isolated postgres");
    assert!(url.ends_with("/tor_ci"), "never test production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let author_email = format!("e05-feed-author-{nonce}@example.invalid");
    let reader_email = format!("e05-feed-reader-{nonce}@example.invalid");
    let outsider_email = format!("e05-feed-outsider-{nonce}@example.invalid");
    let mut actors = Vec::new();
    for email in [author_email, reader_email, outsider_email] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'E05 Feed Fixture','$argon2id$fixture') RETURNING id",
        )
        .bind(email)
        .fetch_one(&pool)
        .await
        .unwrap();
        actors.push(id);
    }
    let author = actors[0];
    let reader = actors[1];
    let outsider = actors[2];

    let mut books = Vec::new();
    for (suffix, title) in [
        ("source", "E05 source book"),
        ("author-only", "E05 author-only book"),
        ("both", "E05 book and author follow"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru',$2,'public',$3) RETURNING id",
        )
        .bind(format!("ci:e05-feed:{nonce}:{suffix}"))
        .bind(title)
        .bind(author)
        .fetch_one(&pool)
        .await
        .unwrap();
        books.push(id);
    }
    let source_book = books[0];
    let author_only_book = books[1];
    let both_follow_book = books[2];
    let anchor: String = sqlx::query_scalar(
        "SELECT (clock_timestamp() - interval '30 seconds')::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Release before the first creator-follow period: it must not be backfilled.
    let before_follow = insert_e05_free_release(
        &pool,
        author_only_book,
        author,
        1,
        "Before creator follow",
        &anchor,
        0,
    )
    .await;
    let period_one: i64 = sqlx::query_scalar(
        "INSERT INTO creator_follow_periods
           (follower_user_id,creator_user_id,source_book_id,started_at,ended_at)
         VALUES ($1,$2,$3,$4::timestamptz + interval '1 second',
                 $4::timestamptz + interval '3 seconds')
         RETURNING id",
    )
    .bind(reader)
    .bind(author)
    .bind(source_book)
    .bind(&anchor)
    .fetch_one(&pool)
    .await
    .unwrap();

    // The reader also follows one book by the same author. A release in this
    // book qualifies through both relations but must be returned only once.
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES ($1,$2,$3::timestamptz + interval '1 second')",
    )
    .bind(reader)
    .bind(both_follow_book)
    .bind(&anchor)
    .execute(&pool)
    .await
    .unwrap();

    let author_only_release = insert_e05_free_release(
        &pool,
        author_only_book,
        author,
        2,
        "After creator follow",
        &anchor,
        2_000_000,
    )
    .await;
    let overlapping_release = insert_e05_free_release(
        &pool,
        both_follow_book,
        author,
        1,
        "Book and creator follow overlap",
        &anchor,
        2_000_000,
    )
    .await;

    // Verified paid release from another book by the followed author.
    let paid_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,3,'Paid author release','public','paid',19900) RETURNING id",
    )
    .bind(author_only_book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Encrypted paid fixture') RETURNING id",
    )
    .bind(paid_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(paid_chapter)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events
           (chapter_id,version_id,creator_user_id,created_at)
         VALUES($1,$2,$3,$4::timestamptz + interval '2.5 seconds')",
    )
    .bind(paid_chapter)
    .bind(paid_version)
    .bind(author)
    .bind(&anchor)
    .execute(&pool)
    .await
    .unwrap();
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
           decode(repeat('11',32),'hex'),sha256(convert_to('Encrypted paid fixture','UTF8')),
           decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),
           convert_to('tor.encrypted.chapter.v1' || E'\\n' || $2::text || E'\\n' ||
              $3::text || E'\\n' || $1::text || E'\\n' ||
              encode(sha256(convert_to('Encrypted paid fixture','UTF8')),'hex') || E'\\n','UTF8'),
           'gzip',now(),now())",
    )
    .bind(&object_id)
    .bind(paid_chapter)
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
    .bind(paid_chapter)
    .bind(paid_version)
    .bind(&object_id)
    .execute(&pool)
    .await
    .unwrap();

    // A release after the first period ended but before the next one began
    // must not be visible to this reader.
    let between_periods = insert_e05_free_release(
        &pool,
        author_only_book,
        author,
        4,
        "Between creator follows",
        &anchor,
        4_000_000,
    )
    .await;
    let period_two: i64 = sqlx::query_scalar(
        "INSERT INTO creator_follow_periods
           (follower_user_id,creator_user_id,source_book_id,started_at)
         VALUES ($1,$2,$3,$4::timestamptz + interval '5 seconds')
         RETURNING id",
    )
    .bind(reader)
    .bind(author)
    .bind(author_only_book)
    .bind(&anchor)
    .fetch_one(&pool)
    .await
    .unwrap();
    let after_refollow = insert_e05_free_release(
        &pool,
        author_only_book,
        author,
        5,
        "After second creator follow",
        &anchor,
        6_000_000,
    )
    .await;

    // Hiding the original source book must not break an account subscription
    // or leak its hidden title into the release feed.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(source_book)
        .execute(&pool)
        .await
        .unwrap();

    let feed = PgFollowUpdates::new(pool.clone());
    let page = feed.latest(reader, None).await.unwrap();
    assert_eq!(page.items.len(), 4);
    let ids: HashSet<i64> = page.items.iter().map(|item| item.chapter_id).collect();
    assert_eq!(ids.len(), 4, "overlapping book/creator follow must deduplicate");
    assert!(!ids.contains(&before_follow));
    assert!(!ids.contains(&between_periods));
    assert!(ids.contains(&author_only_release));
    assert!(ids.contains(&overlapping_release));
    assert!(ids.contains(&paid_chapter));
    assert!(ids.contains(&after_refollow));
    let overlap_count = page
        .items
        .iter()
        .filter(|item| item.chapter_id == overlapping_release)
        .count();
    assert_eq!(overlap_count, 1);

    let paid = page.items.iter().find(|item| item.chapter_id == paid_chapter).unwrap();
    assert!(paid.is_paid);
    assert_eq!(paid.price_minor, 19900);
    assert!(!paid.is_read);
    let unread_before = feed.unread_count_capped(reader).await.unwrap();
    assert_eq!(unread_before, 4);
    assert!(feed.set_read_state(reader, author_only_release, true).await.unwrap());
    assert_eq!(feed.unread_count_capped(reader).await.unwrap(), 3);
    assert!(feed
        .latest(reader, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|item| item.chapter_id == author_only_release && item.is_read));
    assert!(feed.set_read_state(reader, author_only_release, false).await.unwrap());
    assert_eq!(feed.unread_count_capped(reader).await.unwrap(), 4);
    assert!(!feed.set_read_state(outsider, paid_chapter, true).await.unwrap());
    assert!(feed.latest(outsider, None).await.unwrap().items.is_empty());

    let purchases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1",
    )
    .bind(reader)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchases, 0, "author-follow feed must not grant paid access");

    let periods: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id,ended_at::text FROM creator_follow_periods
         WHERE follower_user_id=$1 AND creator_user_id=$2 ORDER BY started_at",
    )
    .bind(reader)
    .bind(author)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(periods.len(), 2);
    assert_eq!(periods[0].0, period_one);
    assert!(periods[0].1.is_some());
    assert_eq!(periods[1].0, period_two);
    assert!(periods[1].1.is_none());
}
