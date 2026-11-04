use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_rec_tickets_recheck_age_after_issue_and_before_click() {
    let (pool, store, actor, book) = fixture("s020-age-ticket").await;
    enable(&store, actor).await;
    let sample = || feed(book);

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let denied = store
        .issue_tickets(actor, &[7; 32], Surface::Home, sample())
        .await
        .unwrap();
    assert!(denied.items.is_empty());

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let mut issued = store
        .issue_tickets(actor, &[7; 32], Surface::Home, sample())
        .await
        .unwrap();
    let adult = issued.items.remove(0).ticket.unwrap();

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.activate(actor, &[7; 32], &adult).await,
        Err(FeedbackError::NotFound)
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.activate(actor, &[7; 32], &adult).await.unwrap(), book);

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let denied = store
        .issue_tickets(actor, &[7; 32], Surface::Home, sample())
        .await
        .unwrap();
    assert!(denied.items.is_empty());

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let mut issued = store
        .issue_tickets(actor, &[7; 32], Surface::Home, sample())
        .await
        .unwrap();
    let pro = issued.items.remove(0).ticket.unwrap();

    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store.activate(actor, &[7; 32], &pro).await,
        Err(FeedbackError::NotFound)
    );
    // A previously recorded click must never become an attribution path after
    // this Book's age entitlement is revoked.
    store.record_book_open(actor, &[7; 32], book).await.unwrap();
    let attributed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_attribution_events e
         JOIN rec_assignments a ON a.id=e.assignment_id
         WHERE a.book_id=$1 AND e.kind='book_opened'",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attributed, 0);
}
