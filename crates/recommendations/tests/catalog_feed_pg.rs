//! Current-schema catalogue, explicit preferences and live ACL behind retirement fences.
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;
use tor_recommendations::{
    domain::feedback::{Choice, FeedbackError},
    repositories::PgRecommendations,
    services::feedback::new_ticket,
};

#[path = "support/current_analytics.rs"]
mod current_analytics;
#[path = "catalog_feed_pg/fixtures.rs"]
mod fixtures;
use fixtures::*;
#[path = "catalog_feed_pg/contextual.rs"]
mod contextual;

async fn fixture(name: &str) -> Fixture {
    let url = current_analytics::url("TEST_DATABASE_URL", "tor_app");
    let pool = PgPool::connect(&url).await.unwrap();
    current_analytics::assert_schema(&pool).await;
    Fixture::new(name, pool).await
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn explicit_hides_are_actor_local_and_catalogue_gets_are_read_only() {
    let f = fixture("preferences").await;
    let actor = f.actors[0];
    let book = f.books[4];
    let key = new_ticket().unwrap();
    f.store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    let state: (String, String) = sqlx::query_as(
        "SELECT p.expires_at::text,c.created_at::text
         FROM rec_book_preferences p JOIN rec_user_preference_commands c
           ON c.user_id=p.user_id AND c.command_type='feedback'
         WHERE p.user_id=$1 AND p.book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let read_pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&current_analytics::url("TEST_DATABASE_URL", "tor_app"))
        .await
        .unwrap();
    let reader = PgRecommendations::new(read_pool.clone());
    let hidden = reader.feed(Some(actor), Some(f.books[0]), 3).await.unwrap();
    assert_catalog(&hidden);
    assert!(!ids(&hidden).contains(&book));
    for viewer in [None, Some(f.actors[1])] {
        let other = reader.feed(viewer, Some(f.books[0]), 3).await.unwrap();
        assert_eq!(ids(&other)[0], book);
        assert_catalog(&other);
    }
    let unchanged: (String, String) = sqlx::query_as(
        "SELECT p.expires_at::text,c.created_at::text
         FROM rec_book_preferences p JOIN rec_user_preference_commands c
           ON c.user_id=p.user_id AND c.command_type='feedback'
         WHERE p.user_id=$1 AND p.book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, state);
    let untouched: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_subjects WHERE user_id=ANY($1::bigint[])),
                (SELECT count(*) FROM rec_preference_accounts WHERE user_id=$2)",
    )
    .bind(f.actors.as_slice())
    .bind(f.actors[1])
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(untouched, (0, 0));
    f.store
        .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    f.store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    let restored = reader.feed(Some(actor), Some(f.books[0]), 3).await.unwrap();
    assert_eq!(ids(&restored)[0], book);
    assert_eq!(restored.items[0].feedback_revision, Some(2));
    read_pool.close().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn catalog_checks_live_book_account_and_age_rights_before_limit() {
    let f = fixture("rights").await;
    let actor = f.actors[0];
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.books[2])
        .execute(&f.pool)
        .await
        .unwrap();
    for (book, tier) in [(f.books[3], "adult"), (f.books[4], "pro_adult")] {
        sqlx::query("UPDATE books SET access_level=$2 WHERE id=$1")
            .bind(book)
            .bind(tier)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    for viewer in [None, Some(actor)] {
        let basic = f.store.feed(viewer, Some(f.books[0]), 1).await.unwrap();
        assert_eq!(ids(&basic), vec![f.books[1]]);
        for restricted in &f.books[2..] {
            assert_eq!(
                f.store.feed(viewer, Some(*restricted), 20).await.err(),
                Some(FeedbackError::NotFound)
            );
        }
    }
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        ids(&f
            .store
            .feed(Some(actor), Some(f.books[0]), 1)
            .await
            .unwrap()),
        vec![f.books[3]]
    );
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',true)",
    )
    .bind(actor)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        ids(&f
            .store
            .feed(Some(actor), Some(f.books[0]), 1)
            .await
            .unwrap()),
        vec![f.books[4]]
    );
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f
        .store
        .feed(Some(actor), None, 20)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        f.store.feed(Some(actor), Some(f.books[0]), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(f.books[0])
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store.feed(None, Some(f.books[0]), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    for (viewer, seed, limit) in [
        (None, None, 0),
        (None, None, 21),
        (Some(0), None, 1),
        (None, Some(0), 1),
    ] {
        assert_eq!(
            f.store.feed(viewer, seed, limit).await.err(),
            Some(FeedbackError::Invalid)
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn catalog_does_not_wait_for_legacy_history_generation_or_rollout_tables() {
    let f = fixture("locked").await;
    let mut legacy = f.pool.begin().await.unwrap();
    sqlx::query(
        "LOCK TABLE rec_subjects,rec_reader_signals,rec_generations,
         rec_candidate_edges,rec_runtime_policy IN ACCESS EXCLUSIVE MODE",
    )
    .execute(&mut *legacy)
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        f.store.feed(Some(f.actors[0]), Some(f.books[0]), 3),
    )
    .await;
    legacy.rollback().await.unwrap();
    let feed = result
        .expect("catalogue GET must not depend on legacy analytics locks")
        .unwrap();
    assert_catalog(&feed);
    assert_eq!(ids(&feed), vec![f.books[4], f.books[3], f.books[2]]);
    f.cleanup().await;
}

#[path = "catalog_feed_pg/contextual_scale.rs"]
mod contextual_scale;
