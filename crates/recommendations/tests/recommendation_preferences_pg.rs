//! Explicit book settings are independent of REC/FLOW collection and its scopes.
use sha2::Digest;
use sqlx::PgPool;
use tor_recommendations::{
    domain::feedback::{Choice, FeedbackError, OrganicSource},
    repositories::PgRecommendations,
    services::feedback::new_ticket,
};

#[path = "support/current_analytics.rs"]
mod current_analytics;
#[path = "recommendation_preferences_pg/fixtures.rs"]
mod fixtures;
use fixtures::*;

async fn fixture(name: &str) -> (PgPool, PgRecommendations, i64, i64) {
    let url = current_analytics::url("TEST_DATABASE_URL", "tor_app");
    let pool = PgPool::connect(&url).await.unwrap();
    current_analytics::assert_schema(&pool).await;
    fixtures::create(pool, name).await
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn explicit_settings_work_without_subject_and_gets_do_not_create_state() {
    let (pool, store, actor, _book) = fixture("off").await;
    assert_eq!(counts(&pool, actor).await, (0, 0, 0, 0));
    let offered = store.feed(Some(actor), None, 20).await.unwrap();
    assert!(!offered.items.is_empty());
    assert!(offered
        .items
        .iter()
        .all(|item| item.feedback_revision == Some(0) && item.ticket.is_none()));
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    let guest = store.feed(None, None, 20).await.unwrap();
    assert!(guest
        .items
        .iter()
        .all(|item| item.feedback_revision.is_none()));
    assert_eq!(counts(&pool, actor).await, (0, 0, 0, 0));

    // Use a card actually returned by the current catalogue feed.
    let book: i64 = offered.items[0].book_id.parse().unwrap();
    let key = new_ticket().unwrap();
    let hidden = store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    assert_eq!(hidden.revision, 1);
    assert_eq!(counts(&pool, actor).await, (1, 1, 1, 0));
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    assert_eq!(
        store
            .issue_organic_tickets(actor, &[7; 32], OrganicSource::Catalog, &[book])
            .await,
        Err(FeedbackError::Forbidden)
    );
    assert!(!store
        .feed(Some(actor), None, 20)
        .await
        .unwrap()
        .items
        .iter()
        .any(|item| item.book_id == book.to_string()));
    assert_eq!(store.hidden_books(actor, 0).await.unwrap()[0].revision, 1);

    store
        .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    let cleared = preference(&pool, actor, book).await.unwrap();
    assert_eq!((cleared.0, cleared.1), (false, 2));
    let restarted = PgRecommendations::new(pool.clone());
    assert_eq!(
        restarted
            .set_feedback(actor, book, Choice::NotInterested, 0, &key)
            .await
            .unwrap(),
        hidden,
        "a replay returns its original result without reapplying an old hide"
    );
    assert_eq!(preference(&pool, actor, book).await.unwrap(), cleared);
    assert!(restarted.hidden_books(actor, 0).await.unwrap().is_empty());
    assert_eq!(counts(&pool, actor).await, (1, 1, 2, 0));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn retired_consent_writes_cannot_change_explicit_choices_or_receipts() {
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
    for enabled in [false, true] {
        assert_eq!(
            store
                .set_consent(actor, enabled, 0, &new_ticket().unwrap())
                .await,
            Err(FeedbackError::Unavailable),
            "the installed fence rejects the old writer without committing state"
        );
        assert_eq!(
            store
                .set_flow_consent(actor, enabled, 0, &new_ticket().unwrap())
                .await,
            Err(FeedbackError::Unavailable)
        );
    }
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    assert_eq!(counts(&pool, actor).await, (1, 1, 1, 0));

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
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn retained_command_keys_and_explicit_payloads_are_isolated_between_actors() {
    let (pool, store, actor, book) = fixture("namespace").await;
    let consent_key = new_ticket().unwrap();
    // A retained explicit command key stays reserved after subject erasure.
    // I's installed-upgrade proof establishes the real migration lifecycle.
    sqlx::query("INSERT INTO rec_preference_accounts(user_id) VALUES($1)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let consent_hash: [u8; 32] = sha2::Sha256::digest("consent:false:0").into();
    sqlx::query(
        "INSERT INTO rec_user_preference_commands
         (user_id,command_key,payload_hash,result_revision,command_type,created_at)
         VALUES($1,$2,$3,1,'consent',now()-interval '1 day')",
    )
    .bind(actor)
    .bind(
        tor_recommendations::domain::feedback::token_digest(&consent_key)
            .unwrap()
            .as_slice(),
    )
    .bind(consent_hash.as_slice())
    .execute(&pool)
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
        store
            .set_feedback(actor, book, Choice::Clear, 1, &feedback_key)
            .await,
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
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn ttl_hides_without_extension_then_erases_and_fences_stale_commands() {
    let (pool, store, actor, book) = fixture("ttl").await;
    let key = new_ticket().unwrap();
    store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '30 minutes'
         WHERE user_id=$1 AND book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let expired = preference(&pool, actor, book).await.unwrap();
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    store.feed(Some(actor), None, 20).await.unwrap();
    store.prune_preferences().await.unwrap();
    assert_eq!(preference(&pool, actor, book).await.unwrap(), expired);
    assert_eq!(epoch(&pool, actor).await, 0);

    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
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
    let restarted = PgRecommendations::new(pool.clone());
    let (left, right) = tokio::join!(store.prune_preferences(), restarted.prune_preferences());
    left.unwrap();
    right.unwrap();
    assert_eq!(counts(&pool, actor).await, (1, 0, 0, 0));
    assert_eq!(epoch(&pool, actor).await, 1, "retention advances once");
    for command in [&key, &new_ticket().unwrap()] {
        assert_eq!(
            store
                .set_feedback(actor, book, Choice::NotInterested, 0, command)
                .await,
            Err(FeedbackError::Conflict)
        );
    }
    let offered = store.feed(Some(actor), None, 20).await.unwrap();
    assert!(!offered.items.is_empty());
    assert!(offered
        .items
        .iter()
        .all(|item| item.feedback_revision == Some(STRIDE)));
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                STRIDE,
                &new_ticket().unwrap()
            )
            .await
            .unwrap()
            .revision,
        STRIDE + 1
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn settings_obey_live_book_and_account_gates_and_user_erasure() {
    let (pool, store, actor, book) = fixture("acl").await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                0,
                &new_ticket().unwrap()
            )
            .await,
        Err(FeedbackError::NotFound)
    );
    assert_eq!(counts(&pool, actor).await, (0, 0, 0, 0));
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
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
    sqlx::query("UPDATE users SET adult_enabled=false WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                1,
                &new_ticket().unwrap()
            )
            .await,
        Err(FeedbackError::NotFound)
    );
    store
        .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    assert!(store
        .feed(Some(actor), None, 20)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 2, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Forbidden)
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, actor).await, (0, 0, 0, 0));
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 0, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Forbidden)
    );
}

#[path = "recommendation_preferences_pg/concurrency.rs"]
mod concurrency;

#[path = "recommendation_preferences_pg/compatibility.rs"]
mod compatibility;
