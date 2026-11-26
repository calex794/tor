//! D-001 persistent SINGLE closure; isolated PostgreSQL only, no S3 deletion.
use super::*;

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_single_seal_survives_reconnect_and_denies_new_obligations() {
    let app = pool().await;
    let url = std::env::var("MIGRATOR_URL").expect("disposable PG18 migrator");
    assert_eq!(
        url,
        "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci"
    );
    let migrator = PgPool::connect(&url).await.unwrap();
    let owner = user(&app, "seal-owner").await;
    let reader = user(&app, "seal-reader").await;
    let book_id = book(&app, owner, "seal-book").await;
    let store = PgFreeS3Publication::new(app.clone());
    let initial = save(book_id, None, 0, "Seal first body", 'U');
    let first = store.prepare_new(owner, &initial).await.unwrap();
    let chapter = first.chapter_id;
    let version = first.version_id;
    store
        .attach_verified_and_publish(owner, book_id, (chapter, version), metadata(&initial.body))
        .await
        .unwrap();
    let request = save(book_id, Some(chapter), 1, "Seal next body", 'V');
    let next = store.prepare_edit(owner, &request).await.unwrap();
    store
        .attach_edit(owner, &request, &next, metadata(&request.body))
        .await
        .unwrap();

    let seal = "SELECT d001_seal_free_single_for_retirement($1,$2)";
    let forbidden = sqlx::query_scalar::<_, bool>(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&app)
        .await
        .unwrap_err();
    assert_eq!(
        forbidden.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let before_observation: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert!(!before_observation);
    let candidates: Vec<i64> = sqlx::query_scalar(
        "SELECT version_id FROM d001_record_free_single_retirement_candidates($1,0,128)",
    )
    .bind(chapter)
    .fetch_all(&app)
    .await
    .unwrap();
    assert_eq!(candidates, vec![version]);
    let current_allowed: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(next.version_id)
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert!(!current_allowed);

    let mut tx = migrator.begin().await.unwrap();
    let closed: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(closed);
    tx.rollback().await.unwrap();
    let open: bool = sqlx::query_scalar(
        "SELECT sealed_at IS NULL FROM d001_free_single_admission_gates
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(open);

    // A writer with an uncommitted pin has already taken a SHARE gate lock.
    // The privileged closer cannot seal until that obligation is resolved.
    let pin = "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
               VALUES($1,$2,$3)";
    let mut writer = app.begin().await.unwrap();
    sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(version)
        .execute(&mut *writer)
        .await
        .unwrap();
    let closed: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert!(!closed);
    writer.rollback().await.unwrap();

    // A transaction with a pre-seal snapshot must not miss a committed
    // closure. Its later FOR SHARE conflicts with the updated gate row.
    let mut stale = app.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale)
        .await
        .unwrap();
    let stale_open: bool = sqlx::query_scalar(
        "SELECT sealed_at IS NULL FROM d001_free_single_admission_gates
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(&mut *stale)
    .await
    .unwrap();
    assert!(stale_open);

    // Commit, close both pools, then use new DB connections. No process memory
    // or session lock carries the admission decision across reconnect.
    let closed: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert!(closed);
    let repeated: bool = sqlx::query_scalar(seal)
        .bind(chapter)
        .bind(version)
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert!(repeated);
    let stale_error = sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(version)
        .execute(&mut *stale)
        .await
        .unwrap_err();
    assert_eq!(
        stale_error.as_database_error().unwrap().code().as_deref(),
        Some("40001")
    );
    stale.rollback().await.unwrap();
    drop(store);
    migrator.close().await;
    app.close().await;

    let app = pool().await;
    let eligible: bool = sqlx::query_scalar(
        "SELECT candidate_now FROM d001_free_single_retirement_queue
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(!eligible);
    for statement in [
        pin,
        "INSERT INTO reader_free_opened_editions(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
        "INSERT INTO reader_free_open_intents
            (user_id,chapter_id,version_id,content_sha256,expires_at)
         SELECT $1,chapter_id,version_id,sha256,now()+interval '5 minutes'
         FROM free_chapter_storage_objects WHERE chapter_id=$2 AND version_id=$3",
    ] {
        let err = sqlx::query(statement)
            .bind(reader)
            .bind(chapter)
            .bind(version)
            .execute(&app)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("23514")
        );
    }
    let (bucket, sha, size) = metadata(&initial.body);
    let delivery: bool =
        sqlx::query_scalar("SELECT d001_record_free_single_delivery($1,$2,$3,$4,$5)")
            .bind(chapter)
            .bind(version)
            .bind(bucket)
            .bind(sha.as_slice())
            .bind(size as i64)
            .fetch_one(&app)
            .await
            .unwrap();
    assert!(!delivery);
    let unseal = sqlx::query(
        "UPDATE d001_free_single_admission_gates SET sealed_at=NULL
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .execute(&app)
    .await
    .unwrap_err();
    assert_eq!(
        unseal.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let replay = PgFreeS3Publication::new(app.clone())
        .prepare_new(owner, &initial)
        .await
        .unwrap();
    assert!(replay.published);
    assert_eq!(counts(&app, chapter).await, (1, 1, 2));
    assert_eq!(published(&app, chapter).await, next.version_id);
    sqlx::query(pin)
        .bind(reader)
        .bind(chapter)
        .bind(next.version_id)
        .execute(&app)
        .await
        .unwrap();
    assert_eq!(counts(&app, chapter).await, (1, 1, 2));
    println!("D001_DURABLE_SINGLE_SEAL_RECONNECT_OK");
    app.close().await;
}
