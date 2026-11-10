use super::*;
use sha2::{Digest, Sha256};
use tor_recommendations::domain::feedback::token_digest;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn copied_public_versions_and_command_receipts_keep_exact_replay_semantics() {
    let (pool, store, actor, book) = fixture("copied").await;
    // The I-owned upgrade test performs the real 189 -> 190 backfill. Here the
    // adapter exercises its exact persisted public CAS/hash/time representation.
    sqlx::query(
        "INSERT INTO rec_preference_accounts(user_id,feedback_epoch) VALUES($1,7)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rec_book_preferences(user_id,book_id,hidden,revision,expires_at)
         VALUES($1,$2,false,9,now()+interval '2 days')",
    )
    .bind(actor)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let old_key = new_ticket().unwrap();
    let expected = 7 * STRIDE + 7;
    let payload: [u8; 32] =
        Sha256::digest(format!("feedback:{book}:true:{expected}")).into();
    sqlx::query(
        "INSERT INTO rec_user_preference_commands
           (user_id,command_key,payload_hash,result_revision,command_type,created_at)
         VALUES($1,$2,$3,$4,'feedback',now()-interval '10 days')",
    )
    .bind(actor)
    .bind(token_digest(&old_key).unwrap().as_slice())
    .bind(payload.as_slice())
    .bind(expected + 1)
    .execute(&pool)
    .await
    .unwrap();
    let before = preference(&pool, actor, book).await.unwrap();
    let stamp: String = sqlx::query_scalar(
        "SELECT created_at::text FROM rec_user_preference_commands WHERE user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    let original = store
        .set_feedback(actor, book, Choice::NotInterested, expected, &old_key)
        .await
        .unwrap();
    assert_eq!((original.revision, original.hidden), (expected + 1, true));
    assert_eq!(preference(&pool, actor, book).await.unwrap(), before);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT created_at::text FROM rec_user_preference_commands WHERE user_id=$1",
        )
        .bind(actor)
        .fetch_one(&pool)
        .await
        .unwrap(),
        stamp
    );
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                expected,
                &new_ticket().unwrap()
            )
            .await,
        Err(FeedbackError::Conflict)
    );

    let consent_key = new_ticket().unwrap();
    let consent_hash: [u8; 32] = Sha256::digest("consent:false:4").into();
    sqlx::query("INSERT INTO rec_subjects(user_id,revision) VALUES($1,5)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rec_user_preference_commands
           (user_id,command_key,payload_hash,result_revision,command_type,created_at)
         VALUES($1,$2,$3,5,'consent',now()-interval '10 days')",
    )
    .bind(actor)
    .bind(token_digest(&consent_key).unwrap().as_slice())
    .bind(consent_hash.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .set_consent(actor, false, 4, &consent_key)
            .await
            .unwrap()
            .revision,
        5
    );
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 7 * STRIDE + 9, &consent_key)
            .await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                7 * STRIDE + 9,
                &new_ticket().unwrap()
            )
            .await
            .unwrap()
            .revision,
        7 * STRIDE + 10
    );
    assert_eq!(epoch(&pool, actor).await, 7);

    // A migrated installation cannot acknowledge new choices in the cleared
    // legacy store while its old binary is waiting for the coordinated restart.
    let error = sqlx::query(
        "INSERT INTO rec_feedback_state(subject_id,book_id,hidden,revision)
         SELECT subject_id,$2,true,1 FROM rec_subjects WHERE user_id=$1",
    )
    .bind(actor)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    for kind in ["feedback", "consent"] {
        let error = sqlx::query(
            "INSERT INTO rec_preference_commands
               (user_id,command_key,payload_hash,result_revision,command_type)
             VALUES($1,$2,$3,0,$4)",
        )
        .bind(actor)
        .bind(vec![0_u8; 32])
        .bind(vec![0_u8; 32])
        .bind(kind)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("55000")
        );
    }
    let old: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM rec_feedback_state f
            JOIN rec_subjects s ON s.subject_id=f.subject_id WHERE s.user_id=$1),
           (SELECT count(*) FROM rec_preference_commands WHERE user_id=$1)",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old, (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn live_exact_replay_survives_another_books_retention_epoch() {
    let (pool, store, actor, expired_book) = fixture("cross-book-epoch").await;
    let (_, _, _, live_book) = fixture("cross-book-live").await;
    store
        .set_feedback(
            actor,
            expired_book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    let live_key = new_ticket().unwrap();
    let original = store
        .set_feedback(actor, live_book, Choice::NotInterested, 0, &live_key)
        .await
        .unwrap();
    let before = preference(&pool, actor, live_book).await.unwrap();
    let receipt: (i64, String) = sqlx::query_as(
        "SELECT result_revision,created_at::text FROM rec_user_preference_commands
         WHERE user_id=$1 AND command_key=$2",
    )
    .bind(actor)
    .bind(token_digest(&live_key).unwrap().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
         WHERE user_id=$1 AND book_id=$2",
    )
    .bind(actor)
    .bind(expired_book)
    .execute(&pool)
    .await
    .unwrap();
    store.prune_preferences().await.unwrap();
    assert!(preference(&pool, actor, expired_book).await.is_none());
    assert_eq!(epoch(&pool, actor).await, 1);
    assert_eq!(
        store
            .set_feedback(actor, live_book, Choice::NotInterested, 0, &live_key)
            .await
            .unwrap(),
        original,
        "an unrelated erasure cannot rewrite a still-live command result"
    );
    assert_eq!(preference(&pool, actor, live_book).await.unwrap(), before);
    let after: (i64, String) = sqlx::query_as(
        "SELECT result_revision,created_at::text FROM rec_user_preference_commands
         WHERE user_id=$1 AND command_key=$2",
    )
    .bind(actor)
    .bind(token_digest(&live_key).unwrap().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, receipt);
    assert_eq!(
        store
            .set_feedback(actor, live_book, Choice::Clear, 1, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Conflict),
        "a new stale command must still use the current epoch"
    );
    let hidden = store.hidden_books(actor, 0).await.unwrap();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].revision, STRIDE + 1);
    store
        .set_feedback(
            actor,
            live_book,
            Choice::Clear,
            STRIDE + 1,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn expired_keys_do_not_replay_and_current_commands_can_reuse_them() {
    let (pool, store, actor, book) = fixture("expired-key").await;
    let key = new_ticket().unwrap();
    store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let before = preference(&pool, actor, book).await.unwrap();
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::NotInterested, 0, &key)
            .await,
        Err(FeedbackError::Conflict),
        "an expired receipt cannot bypass the book CAS"
    );
    assert_eq!(preference(&pool, actor, book).await.unwrap(), before);
    let retained_expired: bool = sqlx::query_scalar(
        "SELECT created_at<now()-interval '30 days' FROM rec_user_preference_commands
         WHERE user_id=$1 AND command_key=$2",
    )
    .bind(actor)
    .bind(token_digest(&key).unwrap().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(retained_expired, "the rejected command rolls back its cleanup");

    let restored = store
        .set_feedback(actor, book, Choice::Clear, 1, &key)
        .await
        .unwrap();
    assert_eq!(restored.revision, 2);
    let cleared = preference(&pool, actor, book).await.unwrap();
    assert_eq!((cleared.0, cleared.1), (false, 2));
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 1, &key)
            .await
            .unwrap(),
        restored
    );
    assert_eq!(preference(&pool, actor, book).await.unwrap(), cleared);

    // The one namespace has the same expiry semantics in both consent APIs.
    sqlx::query(
        "UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .set_consent(actor, false, 0, &key)
            .await
            .unwrap()
            .revision,
        1
    );
    sqlx::query(
        "UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .set_flow_consent(actor, false, 1, &key)
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(preference(&pool, actor, book).await.unwrap(), cleared);
    assert_eq!(
        store.set_feedback(actor, book, Choice::Clear, 2, &key).await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert!(!store.consent(actor).await.unwrap().enabled);
    assert!(store.flow_scope(actor).await.unwrap().is_none());
}
