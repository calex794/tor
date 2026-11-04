use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_organic_tickets_recheck_live_age_before_click_and_book_open() {
    let (pool, store, actor, book) = fixture("s020-organic-tier").await;
    enable(&store, actor).await;
    let session = &[7; 32];
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .issue_organic_tickets(actor, session, OrganicSource::Search, &[book])
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let adult = store
        .issue_organic_tickets(actor, session, OrganicSource::Search, &[book])
        .await
        .unwrap()[0]
        .1
        .clone();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.activate_organic(actor, session, &adult).await,
        Err(FeedbackError::NotFound)
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    store
        .activate_organic(actor, session, &adult)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .issue_organic_tickets(actor, session, OrganicSource::Catalog, &[book])
        .await
        .unwrap()
        .is_empty());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let pro = store
        .issue_organic_tickets(actor, session, OrganicSource::Catalog, &[book])
        .await
        .unwrap()[0]
        .1
        .clone();
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store.activate_organic(actor, session, &pro).await,
        Err(FeedbackError::NotFound)
    );
    store
        .record_organic_book_open(actor, session, book)
        .await
        .unwrap();
    let opened: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organic_discovery_touches
         WHERE book_id=$1 AND book_opened_at IS NOT NULL",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        opened, 0,
        "revoked 21+ grant cannot create book-open attribution"
    );
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .issue_organic_tickets(actor, session, OrganicSource::Catalog, &[book])
            .await,
        Err(FeedbackError::Forbidden),
        "suspended accounts must not issue new organic tickets"
    );
}
