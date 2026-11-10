use super::*;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn exact_book_actions_replay_concurrency_free_pins_and_readd() {
    let pool = pool(4).await;
    let reader = actor(&pool, "actions").await;
    let no_flow = actor(&pool, "rec-only").await;
    consent(&pool, reader, true).await;
    consent(&pool, no_flow, false).await;
    let one = book(&pool, false).await;
    let library = PgLibrary::new(pool.clone()).with_flow_collection(true);
    let follows = PgBookFollows::new(pool.clone()).with_flow_collection(true);

    let (a, b) = tokio::join!(
        library.add_book(reader, one.id),
        library.add_book(reader, one.id)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 1);
    let (a, b) = tokio::join!(
        follows.follow(reader, one.id),
        follows.follow(reader, one.id)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(actions(&pool, reader, 2).await, 1);
    let timestamps_match: bool = sqlx::query_scalar(
        "SELECT bool_and(a.occurred_at=CASE a.kind
           WHEN 1 THEN l.created_at WHEN 2 THEN f.created_at END)
         FROM reader_flow_book_actions a
         JOIN rec_subjects s ON s.subject_id=a.subject_id AND s.flow_epoch=a.flow_epoch
         LEFT JOIN reader_library_items l ON l.user_id=s.user_id AND l.book_id=a.book_id
         LEFT JOIN reader_book_follows f ON f.user_id=s.user_id AND f.book_id=a.book_id
         WHERE s.user_id=$1",
    )
    .bind(reader)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        timestamps_match,
        "event time is the accepted row's created_at"
    );

    // The replay must keep the original pin while also pinning a newly released
    // free chapter. An existing bookmark must not become another save event.
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Second edition') RETURNING id",
    )
    .bind(one.chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(one.chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    let new_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES($1,2,'New free chapter','public','free') RETURNING id",
    )
    .bind(one.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "WITH v AS (
           INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
           VALUES($1,1,'new chapter') RETURNING id)
         INSERT INTO chapter_publications(chapter_id,version_id) SELECT $1,id FROM v",
    )
    .bind(new_chapter)
    .execute(&pool)
    .await
    .unwrap();
    library.add_book(reader, one.id).await.unwrap();
    follows.follow(reader, one.id).await.unwrap();
    let pins: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT chapter_id,version_id FROM reader_saved_free_chapters
         WHERE user_id=$1 ORDER BY chapter_id",
    )
    .bind(reader)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(pins.len(), 2);
    assert!(pins.contains(&(one.chapter, one.version)));
    assert_eq!(actions(&pool, reader, 1).await, 1);
    assert_eq!(actions(&pool, reader, 2).await, 1);

    library.add_book(no_flow, one.id).await.unwrap();
    follows.follow(no_flow, one.id).await.unwrap();
    assert_eq!(actions(&pool, no_flow, 1).await, 0);
    assert_eq!(actions(&pool, no_flow, 2).await, 0);
    consent(&pool, no_flow, true).await;
    library.add_book(no_flow, one.id).await.unwrap();
    follows.follow(no_flow, one.id).await.unwrap();
    assert_eq!(
        actions(&pool, no_flow, 1).await,
        0,
        "no historical backfill"
    );
    assert_eq!(actions(&pool, no_flow, 2).await, 0);

    library.remove_book(reader, one.id).await.unwrap();
    follows.unfollow(reader, one.id).await.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 1, "removal is not a save");
    assert_eq!(actions(&pool, reader, 2).await, 1);
    library.add_book(reader, one.id).await.unwrap();
    follows.follow(reader, one.id).await.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 2);
    assert_eq!(actions(&pool, reader, 2).await, 2);

    let hidden = book(&pool, false).await;
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(hidden.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library.add_book(reader, hidden.id).await.is_err());
    assert!(follows.follow(reader, hidden.id).await.is_err());
    assert_eq!(actions(&pool, reader, 1).await, 2);
    assert_eq!(actions(&pool, reader, 2).await, 2);
    erase_scopes(&pool, &[reader, no_flow]).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn default_off_local_guc_isolation_timestamp_and_transaction_rollback() {
    // Exactly one backend proves SET LOCAL cannot survive pool reuse.
    let pool = pool(1).await;
    let reader = actor(&pool, "local-gate").await;
    consent(&pool, reader, true).await;
    let one = book(&pool, false).await;
    PgLibrary::new(pool.clone())
        .with_flow_collection(true)
        .add_book(reader, one.id)
        .await
        .unwrap();
    let inherited: bool = sqlx::query_scalar(
        "SELECT COALESCE(current_setting('tor.flow_capture_enabled',true),'')='1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!inherited);
    let two = book(&pool, false).await;
    PgLibrary::new(pool.clone())
        .add_book(reader, two.id)
        .await
        .unwrap();
    PgBookFollows::new(pool.clone())
        .follow(reader, two.id)
        .await
        .unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 1);
    assert_eq!(actions(&pool, reader, 2).await, 0);
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(reader)
        .bind(one.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        actions(&pool, reader, 2).await,
        0,
        "SQL trigger defaults off"
    );

    let three = book(&pool, false).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('tor.flow_capture_enabled','1',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Imported/backdated rows cannot be attributed to later consent.
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         VALUES($1,$2,now()-interval '1 hour')",
    )
    .bind(reader)
    .bind(three.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(actions(&pool, reader, 2).await, 0);

    let four = book(&pool, false).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('tor.flow_capture_enabled','1',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reader_library_items(user_id,book_id) VALUES($1,$2)")
        .bind(reader)
        .bind(four.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let inside: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_flow_book_actions WHERE book_id=$1")
            .bind(four.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(inside, 1);
    tx.rollback().await.unwrap();
    let outside: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM reader_library_items WHERE book_id=$1)
              +(SELECT count(*) FROM reader_flow_book_actions WHERE book_id=$1)",
    )
    .bind(four.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outside, 0, "business and optional fact share the rollback");
    erase_scopes(&pool, &[reader]).await;
    pool.close().await;
}
