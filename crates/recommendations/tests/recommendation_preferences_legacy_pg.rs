//! Retained consent transitions on the explicit pre-retirement 191 database.
//! Current settings, CAS, retention, live ACL and catalogue tests remain on 192.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_recommendations::{
    domain::feedback::{token_digest, Choice, FeedbackError},
    repositories::PgRecommendations,
    services::feedback::new_ticket,
};

#[path = "recommendation_preferences_pg/fixtures.rs"]
mod fixtures;
#[path = "support/legacy_analytics.rs"]
mod legacy_analytics;
use fixtures::*;

async fn fixture(name: &str) -> (PgPool, PgRecommendations, i64, i64) {
    let url = legacy_analytics::url("LEGACY_ANALYTICS_DATABASE_URL", "tor_app");
    let pool = PgPool::connect(&url).await.unwrap();
    legacy_analytics::assert_schema(&pool).await;
    fixtures::create(pool, name).await
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn explicit_choices_and_receipts_survive_consent_rotations_and_scope_deletion() {
    let (pool, store, actor, book) = fixture("scope").await;
    let key = new_ticket().unwrap();
    let hidden = store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    let saved = preference(&pool, actor, book).await.unwrap();
    let created: String = sqlx::query_scalar(
        "SELECT created_at::text FROM rec_user_preference_commands
         WHERE user_id=$1 AND command_type='feedback'",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    store
        .set_consent(actor, true, 0, &new_ticket().unwrap())
        .await
        .unwrap();
    store
        .set_flow_consent(actor, true, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    store
        .set_consent(actor, false, 2, &new_ticket().unwrap())
        .await
        .unwrap();
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    store
        .set_consent(actor, true, 3, &new_ticket().unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM rec_subjects WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(epoch(&pool, actor).await, 0);
    assert_eq!(preference(&pool, actor, book).await.unwrap(), saved);
    assert_eq!(store.hidden_books(actor, 0).await.unwrap()[0].revision, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT created_at::text FROM rec_user_preference_commands
             WHERE user_id=$1 AND command_type='feedback'",
        )
        .bind(actor)
        .fetch_one(&pool)
        .await
        .unwrap(),
        created
    );
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::NotInterested, 0, &key)
            .await
            .unwrap(),
        hidden
    );
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 0, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Conflict)
    );
    assert!(!store
        .feed(Some(actor), None, 20)
        .await
        .unwrap()
        .items
        .iter()
        .any(|item| item.book_id == book.to_string()));
    assert_eq!(counts(&pool, actor).await.3, 0);
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn command_namespace_is_shared_across_kinds_and_isolated_between_actors() {
    let (pool, store, actor, book) = fixture("namespace").await;
    let consent_key = new_ticket().unwrap();
    store
        .set_consent(actor, false, 0, &consent_key)
        .await
        .unwrap();
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::NotInterested, 0, &consent_key)
            .await,
        Err(FeedbackError::IdempotencyConflict)
    );
    let feedback_key = new_ticket().unwrap();
    store
        .set_feedback(actor, book, Choice::NotInterested, 0, &feedback_key)
        .await
        .unwrap();
    assert_eq!(
        store.set_consent(actor, true, 1, &feedback_key).await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert_eq!(
        store.set_flow_consent(actor, false, 1, &feedback_key).await,
        Err(FeedbackError::IdempotencyConflict)
    );
    let (_, other_store, other, _) = fixture("namespace-other").await;
    other_store
        .set_feedback(other, book, Choice::NotInterested, 0, &feedback_key)
        .await
        .unwrap();
    other_store
        .set_feedback(other, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    assert!(other_store.hidden_books(other, 0).await.unwrap().is_empty());
    assert_eq!(store.hidden_books(actor, 0).await.unwrap()[0].revision, 1);
    assert!(preference(&pool, actor, book).await.unwrap().0);
    assert!(!store.consent(actor).await.unwrap().enabled);
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn two_adapters_serialize_replay_cas_and_cross_kind_commands() {
    let (pool, store, actor, book) = fixture("concurrency").await;
    let other = PgRecommendations::new(pool.clone());
    let hide = new_ticket().unwrap();
    let (left, right) = tokio::join!(
        store.set_feedback(actor, book, Choice::NotInterested, 0, &hide),
        other.set_feedback(actor, book, Choice::NotInterested, 0, &hide)
    );
    let receipt = left.unwrap();
    assert_eq!(receipt, right.unwrap());
    assert_eq!(counts(&pool, actor).await, (1, 1, 1, 0));
    assert_eq!(preference(&pool, actor, book).await.unwrap().1, 1);

    let left_key = new_ticket().unwrap();
    let right_key = new_ticket().unwrap();
    let (left, right) = tokio::join!(
        store.set_feedback(actor, book, Choice::Clear, 1, &left_key),
        other.set_feedback(actor, book, Choice::Clear, 1, &right_key)
    );
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(FeedbackError::Conflict)))
            .count(),
        1
    );
    assert_eq!(counts(&pool, actor).await, (1, 1, 2, 0));
    assert_eq!(preference(&pool, actor, book).await.unwrap().1, 2);

    let shared = new_ticket().unwrap();
    let (consent, feedback) = tokio::join!(
        store.set_consent(actor, false, 0, &shared),
        other.set_feedback(actor, book, Choice::NotInterested, 2, &shared)
    );
    assert!(
        consent.is_ok() && feedback == Err(FeedbackError::IdempotencyConflict)
            || feedback.is_ok() && consent == Err(FeedbackError::IdempotencyConflict),
        "different command kinds must share one atomic namespace"
    );
    assert_eq!(counts(&pool, actor).await.2, 3);
    assert!(!store.consent(actor).await.unwrap().enabled);
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn copied_consent_receipt_replays_and_reserves_its_original_key() {
    let (pool, store, actor, book) = fixture("copied-consent").await;
    sqlx::query("INSERT INTO rec_preference_accounts(user_id,feedback_epoch) VALUES($1,7)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rec_subjects(user_id,revision) VALUES($1,5)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let key = new_ticket().unwrap();
    let payload: [u8; 32] = Sha256::digest("consent:false:4").into();
    sqlx::query(
        "INSERT INTO rec_user_preference_commands
         (user_id,command_key,payload_hash,result_revision,command_type,created_at)
         VALUES($1,$2,$3,5,'consent',now()-interval '10 days')",
    )
    .bind(actor)
    .bind(token_digest(&key).unwrap().as_slice())
    .bind(payload.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let stamp: String = sqlx::query_scalar(
        "SELECT created_at::text FROM rec_user_preference_commands WHERE user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .set_consent(actor, false, 4, &key)
            .await
            .unwrap()
            .revision,
        5
    );
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 7 * STRIDE, &key)
            .await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert_eq!(epoch(&pool, actor).await, 7);
    let after: String = sqlx::query_scalar(
        "SELECT created_at::text FROM rec_user_preference_commands WHERE user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stamp, after);
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn expired_explicit_key_can_be_reused_by_both_legacy_consent_commands() {
    let (pool, store, actor, book) = fixture("expired-consent-key").await;
    store
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    let key = new_ticket().unwrap();
    store
        .set_feedback(actor, book, Choice::Clear, 1, &key)
        .await
        .unwrap();
    let cleared = preference(&pool, actor, book).await.unwrap();
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
        store
            .set_feedback(actor, book, Choice::Clear, 2, &key)
            .await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert!(!store.consent(actor).await.unwrap().enabled);
    assert!(store.flow_scope(actor).await.unwrap().is_none());
}
