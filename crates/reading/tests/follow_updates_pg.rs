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
