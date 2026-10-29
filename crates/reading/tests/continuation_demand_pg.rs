//! Loop 1 / C: tested against disposable PostgreSQL 18 only.
use sqlx::PgPool;
use tor_reading::repositories::demand::{DemandState, PgContinuationDemand};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL tor_ci"]
async fn c_14_demand_is_explicit_deduplicated_and_bound_to_released_frontier() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG");
    assert!(url.ends_with("/tor_ci"), "no production");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM books WHERE id=99410")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=99401")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(id,email_normalized,display_name,password_hash) OVERRIDING SYSTEM VALUE VALUES(99401,'c14-demand@example.invalid','Reader','$argon2id$fixture')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO books(id,source_identity,target_language,title,visibility) VALUES(99410,'ci:c14:demand','ru','Book','public')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount) VALUES (99411,99410,1,'One','public','free',0),(99412,99410,2,'Two','draft','free',0)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown) VALUES(99421,99411,1,'End'),(99422,99412,1,'Future')").execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES(99411,99421),(99412,99422)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let demand = PgContinuationDemand::new(pool.clone());
    assert_eq!(
        demand.state(99401, 99410, 99411).await.unwrap(),
        DemandState::NotEligible
    );
    assert!(demand
        .record_verified_continuation(99401, 99410, 99411)
        .await
        .is_err());
    sqlx::query("INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,source_event_id) VALUES(99401,99410,99411,99421,gen_random_uuid())").execute(&pool).await.unwrap();
    assert_eq!(
        demand.state(99401, 99410, 99411).await.unwrap(),
        DemandState::Ready
    );
    demand
        .record_verified_continuation(99401, 99410, 99411)
        .await
        .unwrap();
    demand
        .record_verified_continuation(99401, 99410, 99411)
        .await
        .unwrap();
    assert_eq!(
        demand.state(99401, 99410, 99411).await.unwrap(),
        DemandState::Recorded
    );
    assert_eq!(demand.current_count(99410).await.unwrap(), 1);
    sqlx::query("UPDATE chapters SET visibility='public',access_level='paid',price_amount=100 WHERE id=99412").execute(&pool).await.unwrap();
    assert_eq!(
        demand.state(99401, 99410, 99411).await.unwrap(),
        DemandState::NotEligible
    );
    assert_eq!(demand.current_count(99410).await.unwrap(), 0);
    assert!(demand
        .record_verified_continuation(99401, 99410, 99411)
        .await
        .is_err());
    sqlx::query("DELETE FROM books WHERE id=99410")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=99401")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
