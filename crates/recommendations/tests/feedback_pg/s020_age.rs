//! S020: actor age revocation and recommendation ticket regression tests.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_suspended_rec_actor_cannot_reuse_private_consent_subject() {
    let (pool, store, actor, _) = fixture("s020-suspended-subject").await;
    enable(&store, actor).await;
    let active = store.feed(Some(actor), None, 20).await.unwrap();
    assert!(
        active
            .items
            .iter()
            .any(|item| item.feedback_revision.is_some()),
        "active consent must produce versioned recommendation cards"
    );
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let consent_still_enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM rec_subjects WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consent_still_enabled, "suspension is not consent deletion");
    let revoked = store.feed(Some(actor), None, 20).await.unwrap();
    assert!(!revoked.personalized);
    assert!(
        revoked
            .items
            .iter()
            .all(|item| item.feedback_revision.is_none()),
        "stale actor IDs may only receive anonymous public metadata"
    );
}

