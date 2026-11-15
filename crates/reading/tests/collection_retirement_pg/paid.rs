use super::*;

#[tokio::test]
#[ignore = "requires current disposable PostgreSQL tor_ci at migration 192"]
async fn paid_archived_and_verified_s3_commands_keep_entitlements_and_immutable_events() {
    let pool = pool(3).await;
    let reader = actor(&pool, "paid").await;
    let outsider = actor(&pool, "outsider").await;
    let baseline = optional_counts(&pool).await;
    let sync = legacy_sync(&pool);
    let paid = book(&pool, true).await;
    let purchase_id = purchase(&pool, reader, paid).await;
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
    assert_canonical(&pool, reader, paid, Some(purchase_id)).await;
    let direct_snapshot = canonical_snapshot(&pool, reader).await;
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'ci:retired-c-refund:'||gen_random_uuid(),'refund')",
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
    assert_eq!(
        canonical_snapshot(&pool, reader).await,
        direct_snapshot,
        "revocation stops access without rewriting accepted event evidence"
    );

    let archived = book(&pool, false).await;
    PgLibrary::new(pool.clone())
        .with_flow_collection(true)
        .add_book(reader, archived.id)
        .await
        .unwrap();
    let mut session = [0_u8; 32];
    session[..8].copy_from_slice(&reader.to_le_bytes());
    session[31] = 51;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(reader)
    .bind(session.as_slice())
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
    let before_archive = canonical_counts(&pool, reader).await;
    sync.save_archived_free(
        reader,
        archived.chapter,
        archived.version,
        place(10),
        0,
        &session,
    )
    .await
    .unwrap();
    assert_eq!(
        canonical_counts(&pool, reader).await,
        before_archive,
        "archived free continuation saves the pinned cursor without inventing a canonical event"
    );
    let pinned_position: (i64, i64, i32, i32, i64) = sqlx::query_as(
        "SELECT chapter_id,version_id,block_index,char_offset,revision
         FROM reader_book_positions WHERE user_id=$1 AND book_id=$2",
    )
    .bind(reader)
    .bind(archived.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        pinned_position,
        (archived.chapter, archived.version, 1, 10, 1)
    );
    let archive_snapshot = canonical_snapshot(&pool, reader).await;
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(session.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_archived_free(
            reader,
            archived.chapter,
            archived.version,
            place(10),
            1,
            &session
        )
        .await,
        Err(SyncError::NotFound)
    );
    assert_eq!(canonical_snapshot(&pool, reader).await, archive_snapshot);

    let s3 = book(&pool, true).await;
    let hash = encrypted(&pool, s3).await;
    let s3_purchase = purchase(&pool, reader, s3).await;
    let counts = canonical_counts(&pool, reader).await;
    sync.save_purchased(reader, s3_purchase, s3.chapter, s3.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        canonical_counts(&pool, reader).await,
        counts,
        "an encrypted shadow cursor is not verified reading"
    );
    let command = |digest| PurchasedFinish {
        purchase_id: s3_purchase,
        chapter_id: s3.chapter,
        version_id: s3.version,
        place: place(10),
        expected_revision: 1,
        plaintext_sha256: digest,
    };
    assert_eq!(
        sync.complete_purchased_s3(reader, command("wrong")).await,
        Err(SyncError::Invalid)
    );
    assert_eq!(
        sync.complete_purchased_s3(outsider, command(&hash)).await,
        Err(SyncError::NotFound)
    );
    let (event, committed) = sync
        .complete_purchased_s3(reader, command(&hash))
        .await
        .unwrap();
    assert!(committed);
    assert_canonical(&pool, reader, s3, Some(s3_purchase)).await;
    let completed_snapshot = canonical_snapshot(&pool, reader).await;
    assert_eq!(
        sync.complete_purchased_s3(reader, command(&hash))
            .await
            .unwrap(),
        (event, false)
    );
    assert_eq!(
        canonical_snapshot(&pool, reader).await,
        completed_snapshot,
        "S3 receipt replay preserves its exact event, payload and first version"
    );
    assert_eq!(canonical_counts(&pool, outsider).await, (0, 0));
    assert_no_optional(&pool, reader, &baseline).await;
    assert_no_optional(&pool, outsider, &baseline).await;
    pool.close().await;
}
