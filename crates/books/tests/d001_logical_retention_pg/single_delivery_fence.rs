//! D-001: real PG transaction races. No persistent seal or S3 I/O.
use super::*;

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_single_delivery_and_pins_conflict_with_retirement_fence() {
    let pool = pool().await;
    let owner = user(&pool, "fence-owner").await;
    let reader = user(&pool, "fence-reader").await;
    let book_id = book(&pool, owner, "fence-book").await;
    let store = PgFreeS3Publication::new(pool.clone());
    let initial = save(book_id, None, 0, "Fence first body", 'X');
    let first = store.prepare_new(owner, &initial).await.unwrap();
    let chapter = first.chapter_id;
    let version = first.version_id;
    store
        .attach_verified_and_publish(owner, book_id, (chapter, version), metadata(&initial.body))
        .await
        .unwrap();
    let request = save(book_id, Some(chapter), 1, "Fence second body", 'Y');
    let second = store.prepare_edit(owner, &request).await.unwrap();
    store
        .attach_edit(owner, &request, &second, metadata(&request.body))
        .await
        .unwrap();

    let record = "SELECT version_id FROM d001_record_free_single_retirement_candidates($1,0,128)";
    let candidates: Vec<i64> = sqlx::query_scalar(record)
        .bind(chapter)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(candidates, vec![version]);

    let fence = "SELECT d001_try_lock_free_single_retirement($1,$2)";
    let pin = "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
               VALUES($1,$2,$3)";
    let delivery = "SELECT d001_record_free_single_delivery($1,$2,$3,$4,$5)";
    let (bucket, sha, size) = metadata(&initial.body);

    // Uncommitted FK pin holds KEY SHARE on this exact version. A NOWAIT
    // retirement probe must fail, not mistake the invisible pin for absence.
    let mut writer = pool.begin().await.unwrap();
    sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(version)
        .execute(&mut *writer)
        .await
        .unwrap();
    let admitted: bool = sqlx::query_scalar(fence)
        .bind(chapter)
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!admitted);
    writer.rollback().await.unwrap();

    // Reverse ordering: while the probe owns the exact version fence, neither
    // a new FK pin nor a signed-delivery registration can complete.
    let mut retirement = pool.begin().await.unwrap();
    let admitted: bool = sqlx::query_scalar(fence)
        .bind(chapter)
        .bind(version)
        .fetch_one(&mut *retirement)
        .await
        .unwrap();
    assert!(admitted);
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *writer)
        .await
        .unwrap();
    let err = sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(version)
        .execute(&mut *writer)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    writer.rollback().await.unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *writer)
        .await
        .unwrap();
    let err = sqlx::query_scalar::<_, bool>(delivery)
        .bind(chapter)
        .bind(version)
        .bind(bucket)
        .bind(sha.as_slice())
        .bind(size as i64)
        .fetch_one(&mut *writer)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    writer.rollback().await.unwrap();
    retirement.rollback().await.unwrap();

    // A forged digest is never accepted as an exact delivery.
    let forged: bool = sqlx::query_scalar(delivery)
        .bind(chapter)
        .bind(version)
        .bind(bucket)
        .bind([0_u8; 32].as_slice())
        .bind(size as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!forged);
    let recorded: bool = sqlx::query_scalar(delivery)
        .bind(chapter)
        .bind(version)
        .bind(bucket)
        .bind(sha.as_slice())
        .bind(size as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(recorded);
    let protected: bool = sqlx::query_scalar(
        "SELECT protected_until>clock_timestamp()+interval '4 minutes'
         FROM d001_free_single_delivery_holds WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(protected);
    let admitted: bool = sqlx::query_scalar(fence)
        .bind(chapter)
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!admitted);

    // The probe never permanently closes the version; after rollback the
    // reader can legitimately pin it. No read or retention right is changed.
    sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (2, 2, 2));
    assert_eq!(published(&pool, chapter).await, second.version_id);
    println!("D001_SINGLE_DELIVERY_FENCE_OK");
    pool.close().await;
}
