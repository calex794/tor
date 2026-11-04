//! S020/P0: feedback, hidden preferences and impression receipts obey live B age gates.
//! All mutations use the isolated tor_ci fixture from the parent integration suite.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_adult_feedback_and_hidden_titles_follow_live_opt_in() {
    let (pool, store, actor, book) = fixture("s020-feedback-adult").await;
    enable(&store, actor).await;
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
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();

    let saved = store
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    let hidden = store.hidden_books(actor, 0).await.unwrap();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].book_id, book.to_string());

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store.hidden_books(actor, 0).await.unwrap().is_empty(),
        "revoked age access cannot expose a previously hidden Book title"
    );
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

    // Allow clearing an existing preference without re-granting access
    // or returning a hidden Book title to the revoked reader.
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
            .await
            .unwrap()
            .revision,
        2
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    store
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            2,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
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
                3,
                &new_ticket().unwrap()
            )
            .await,
        Err(FeedbackError::Forbidden)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_pro_adult_impressions_recheck_active_grant_and_legacy_null() {
    let (pool, store, actor, book) = fixture("s020-feedback-pro-impression").await;
    enable(&store, actor).await;
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
        .await
        .unwrap()
        .items
        .is_empty());

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let ticket = store
        .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
        .await
        .unwrap()
        .items
        .remove(0)
        .ticket
        .unwrap();
    store
        .impressions(actor, &[7; 32], &[impression(&ticket, true)])
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
    assert_eq!(store.hidden_books(actor, 0).await.unwrap().len(), 1);

    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .impressions(actor, &[7; 32], &[impression(&ticket, true)])
            .await,
        Err(FeedbackError::NotFound)
    );
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());

    sqlx::query("UPDATE user_level_grants SET is_active=TRUE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    store
        .impressions(actor, &[7; 32], &[impression(&ticket, true)])
        .await
        .unwrap();
    assert_eq!(store.hidden_books(actor, 0).await.unwrap().len(), 1);
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.hidden_books(actor, 0).await.unwrap().is_empty());
    assert_eq!(
        store
            .impressions(actor, &[7; 32], &[impression(&ticket, true)])
            .await,
        Err(FeedbackError::NotFound)
    );
}
