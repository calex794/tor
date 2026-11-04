use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_adult_and_pro_feedback_impressions_recheck_current_actor_entitlement() {
    for (name, level) in [
        ("s020-adult-feedback", "adult"),
        ("s020-pro-feedback", "pro_adult"),
    ] {
        let (pool, store, actor, book) = fixture(name).await;
        enable(&store, actor).await;
        sqlx::query("UPDATE books SET access_level=$1 WHERE id=$2")
            .bind(level)
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

        if level == "adult" {
            sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
                .bind(actor)
                .execute(&pool)
                .await
                .unwrap();
        } else {
            sqlx::query(
                "INSERT INTO user_level_grants(user_id,level,source,is_active)
                 VALUES($1,'pro_adult','moderation',TRUE)",
            )
            .bind(actor)
            .execute(&pool)
            .await
            .unwrap();
        }
        let ticket = assignment(&store, actor, book).await;
        store
            .impressions(actor, &[7; 32], &[impression(&ticket, true)])
            .await
            .unwrap();
        let viewed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rec_attribution_events e
             JOIN rec_assignments a ON a.id=e.assignment_id
             WHERE a.book_id=$1 AND e.kind='impression'",
        )
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(viewed, 1, "{level} must accept a genuine impression");

        let hidden = store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                0,
                &new_ticket().unwrap(),
            )
            .await
            .unwrap();
        assert!(hidden.hidden);
        assert!(store
            .hidden_books(actor, 0)
            .await
            .unwrap()
            .iter()
            .any(|row| row.book_id == book.to_string()));

        if level == "adult" {
            sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
                .bind(actor)
                .execute(&pool)
                .await
                .unwrap();
        } else {
            sqlx::query(
                "UPDATE user_level_grants SET is_active=FALSE
                 WHERE user_id=$1 AND level='pro_adult'",
            )
            .bind(actor)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(
            store
                .impressions(actor, &[7; 32], &[impression(&ticket, true)])
                .await,
            Err(FeedbackError::NotFound)
        );
        assert!(store
            .hidden_books(actor, 0)
            .await
            .unwrap()
            .iter()
            .all(|row| row.book_id != book.to_string()));
    }
}
