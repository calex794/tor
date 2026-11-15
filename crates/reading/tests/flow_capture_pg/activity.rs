use super::*;

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn positive_progress_only_independent_of_old_return_markers_and_scope_rotation() {
    let pool = pool(4).await;
    let reader = actor(&pool, "positive").await;
    let (subject, _) = consent(&pool, reader, true).await;
    let one = book(&pool, false).await;
    let sync = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    let manual = PgReadingProgress::new(pool.clone()).with_flow_collection(true);
    // A finished global D30 cohort must not stop unrelated FLOW attribution.
    sqlx::query("INSERT INTO reader_return_subjects(subject_id) VALUES($1::uuid)")
        .bind(&subject)
        .execute(&pool)
        .await
        .unwrap();
    assert!(PgReader::new(pool.clone())
        .public_chapter(one.chapter as u64)
        .await
        .unwrap()
        .is_some());
    assert!(sync
        .free_state(reader, one.chapter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        days(&pool, reader).await,
        0,
        "read-only access is not reading"
    );
    sync.save_free(reader, one.chapter, one.version, place(0), 0)
        .await
        .unwrap();
    assert_eq!(
        days(&pool, reader).await,
        0,
        "initial anchor is not progress"
    );
    assert_eq!(
        sync.save_free(reader, one.chapter, one.version, place(99), 1)
            .await,
        Err(SyncError::Invalid)
    );
    assert_eq!(days(&pool, reader).await, 0);
    sync.save_free(reader, one.chapter, one.version, place(1), 1)
        .await
        .unwrap();
    assert_eq!(days(&pool, reader).await, 1);
    manual
        .save_position(reader, one.chapter, one.version, place(2))
        .await
        .unwrap();
    assert_eq!(
        days(&pool, reader).await,
        1,
        "same scope/book/day deduplicates"
    );
    let old_returns: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_activity_days WHERE subject_id=$1::uuid")
            .bind(&subject)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_returns, 0, "old collectors remain disabled");

    PgLibrary::new(pool.clone())
        .with_flow_collection(true)
        .add_book(reader, one.id)
        .await
        .unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 1);
    consent(&pool, reader, false).await;
    let erased: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM reader_flow_activity_days WHERE subject_id=$1::uuid)
              +(SELECT count(*) FROM reader_flow_book_actions WHERE subject_id=$1::uuid)",
    )
    .bind(&subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(erased, 0, "FLOW withdrawal erases both C fact types");
    consent(&pool, reader, true).await;
    manual
        .save_position(reader, one.chapter, one.version, place(2))
        .await
        .unwrap();
    assert_eq!(
        days(&pool, reader).await,
        0,
        "a successful no-op cannot backfill"
    );
    manual
        .save_position(reader, one.chapter, one.version, place(3))
        .await
        .unwrap();
    assert_eq!(days(&pool, reader).await, 1);
    // Move this genuine fact to the preceding UTC day to exercise daily keys
    // without waiting a day; the next actual advance must add today's fact.
    sqlx::query(
        "UPDATE reader_flow_activity_days SET activity_day=activity_day-1,
           first_seen_at=first_seen_at-interval '1 day' WHERE subject_id=$1::uuid",
    )
    .bind(&subject)
    .execute(&pool)
    .await
    .unwrap();
    manual
        .save_position(reader, one.chapter, one.version, place(4))
        .await
        .unwrap();
    assert_eq!(days(&pool, reader).await, 2);

    let disabled = actor(&pool, "disabled").await;
    consent(&pool, disabled, true).await;
    PgReadingSync::new(pool.clone())
        .save_free(disabled, one.chapter, one.version, place(2), 0)
        .await
        .unwrap();
    PgReadingProgress::new(pool.clone())
        .save_position(disabled, one.chapter, one.version, place(3))
        .await
        .unwrap();
    assert_eq!(days(&pool, disabled).await, 0);

    let rec_only = actor(&pool, "progress-rec-only").await;
    consent(&pool, rec_only, false).await;
    sync.save_free(rec_only, one.chapter, one.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(days(&pool, rec_only).await, 0);
    // Start a real C transaction before re-consent, holding its publication
    // lock until the new epoch exists. Its accepted position retains that
    // earlier transaction timestamp and must not be moved into the new epoch.
    let boundary = book(&pool, false).await;
    consent(&pool, rec_only, true).await;
    let mut locked = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM chapter_versions WHERE id=$1 FOR UPDATE")
        .bind(boundary.version)
        .execute(&mut *locked)
        .await
        .unwrap();
    let in_flight = sync.clone();
    let pending = tokio::spawn(async move {
        in_flight
            .save_free(rec_only, boundary.chapter, boundary.version, place(1), 0)
            .await
    });
    let mut waiting = false;
    for _ in 0..100 {
        waiting = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
        )
        .bind(blocker)
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(waiting, "the real C transaction must precede re-consent");
    consent(&pool, rec_only, true).await;
    locked.rollback().await.unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(days(&pool, rec_only).await, 0);
    assert!(reads(&pool, rec_only, boundary.id).await.is_empty());
    sync.save_free(rec_only, boundary.chapter, boundary.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(days(&pool, rec_only).await, 1);
    erase_scopes(&pool, &[reader, disabled, rec_only]).await;
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_activity_days WHERE subject_id=$1::uuid",
    )
    .bind(&subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        remaining, 0,
        "account subject deletion cannot retain C scope rows"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn direct_paid_archived_and_verified_s3_paths_keep_their_existing_acl() {
    let pool = pool(4).await;
    let reader = actor(&pool, "paid").await;
    let outsider = actor(&pool, "paid-outsider").await;
    consent(&pool, reader, true).await;
    consent(&pool, outsider, true).await;
    let sync = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    let paid = book(&pool, true).await;
    let purchase_id = purchase(&pool, reader, paid).await;
    assert!(sync
        .purchased_state(reader, purchase_id, paid.chapter, paid.version)
        .await
        .unwrap()
        .is_none());
    assert_eq!(days(&pool, reader).await, 0);
    sync.save_purchased(
        reader,
        purchase_id,
        paid.chapter,
        paid.version,
        place(10),
        0,
    )
    .await
    .unwrap();
    assert_eq!(days(&pool, reader).await, 1);
    assert_eq!(
        sync.save_purchased(
            outsider,
            purchase_id,
            paid.chapter,
            paid.version,
            place(2),
            0
        )
        .await,
        Err(SyncError::NotFound)
    );
    assert_eq!(days(&pool, outsider).await, 0);
    let historical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_verified_read_events
         WHERE user_id=$1 AND book_id=$2",
    )
    .bind(reader)
    .bind(paid.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        historical, 2,
        "canonical start and completion both have matching outbox evidence"
    );
    let first: bool = sqlx::query_scalar(
        "SELECT first_for_book FROM reader_flow_verified_read_events
         WHERE user_id=$1 AND book_id=$2 AND kind=0",
    )
    .bind(reader)
    .bind(paid.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        first,
        "simultaneous first completion cannot erase the first start"
    );
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'ci:flow-c-refund:'||gen_random_uuid(),'refund')",
    )
    .bind(purchase_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        sync.save_purchased(
            reader,
            purchase_id,
            paid.chapter,
            paid.version,
            place(10),
            1
        )
        .await,
        Err(SyncError::NotFound)
    );
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(paid.id)
        .execute(&pool)
        .await
        .unwrap();
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_verified_read_events WHERE user_id=$1 AND book_id=$2",
    )
    .bind(reader)
    .bind(paid.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        retained, historical,
        "later access loss must not rewrite authorized C history"
    );

    let archived = book(&pool, false).await;
    PgLibrary::new(pool.clone())
        .add_book(reader, archived.id)
        .await
        .unwrap();
    let mut digest = [0_u8; 32];
    digest[..8].copy_from_slice(&reader.to_le_bytes());
    digest[31] = 37;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(reader)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(archived.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='ordinary_deleted',ordinary_deleted_at=now(),visibility='draft' WHERE id=$1")
        .bind(archived.id).execute(&pool).await.unwrap();
    sync.save_archived_free(
        reader,
        archived.chapter,
        archived.version,
        place(2),
        0,
        &digest,
    )
    .await
    .unwrap();
    assert_eq!(days(&pool, reader).await, 2);
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_archived_free(
            reader,
            archived.chapter,
            archived.version,
            place(3),
            1,
            &digest
        )
        .await,
        Err(SyncError::NotFound)
    );
    assert_eq!(days(&pool, reader).await, 2);

    let s3 = book(&pool, true).await;
    let hash = encrypted(&pool, s3).await;
    let s3_purchase = purchase(&pool, reader, s3).await;
    sync.save_purchased(reader, s3_purchase, s3.chapter, s3.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        days(&pool, reader).await,
        2,
        "S3 shadow cursor is not trusted reading"
    );
    let command = || PurchasedFinish {
        purchase_id: s3_purchase,
        chapter_id: s3.chapter,
        version_id: s3.version,
        place: place(10),
        expected_revision: 1,
        plaintext_sha256: &hash,
    };
    assert!(
        sync.complete_purchased_s3(reader, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(days(&pool, reader).await, 3);
    consent(&pool, reader, false).await;
    consent(&pool, reader, true).await;
    assert!(
        !sync
            .complete_purchased_s3(reader, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(
        days(&pool, reader).await,
        0,
        "completion replay cannot enroll the new scope"
    );
    erase_scopes(&pool, &[reader, outsider]).await;
    pool.close().await;
}
