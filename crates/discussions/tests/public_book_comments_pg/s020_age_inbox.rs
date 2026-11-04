//! S020: isolated age-tier and Book-moderation inbox regression.
//! Invoked from mandatory e_02_e03_received_replies_are_private_live_references.
use sqlx::PgPool;
use tor_discussions::{
    repositories::public_book_comments::{PgPublicBookComments, ReadStateResult},
    services::inbox::{InboxFilter, InboxService},
};

pub(super) async fn verify(
    pool: &PgPool,
    repo: &PgPublicBookComments,
    inbox: &InboxService,
    book: i64,
    actor: i64,
    second: i64,
) {
    // S020: the private reply inbox is a metadata delivery surface, not a
    // bypass around current Book age/site eligibility. Preserve the replies
    // and read flags while visibility changes; restore them on reauthorization.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    assert!(inbox
        .list(actor, InboxFilter::All, None, 25)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        repo.set_reply_read_state(actor, second, true).await,
        ReadStateResult::NotFound
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);
    assert_eq!(
        inbox
            .list(actor, InboxFilter::All, None, 25)
            .await
            .unwrap()
            .items
            .len(),
        2
    );

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    // NEwBOO grants pro_adult independently of the adult_enabled switch.
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);
    assert_eq!(
        inbox
            .list(actor, InboxFilter::All, None, 25)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        repo.set_reply_read_state(actor, second, true).await,
        ReadStateResult::Updated
    );
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    assert!(inbox
        .list(actor, InboxFilter::All, None, 25)
        .await
        .unwrap()
        .items
        .is_empty());

    // An unknown legacy tier fails closed, but a live site administrator
    // retains the separate NEwBOO privileged role; site moderators do not.
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);
    assert_eq!(
        inbox
            .list(actor, InboxFilter::All, None, 25)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'moderator',TRUE)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='moderator'")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();

    sqlx::query("UPDATE books SET access_level='basic',visibility_status='hidden' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert!(inbox
        .list(actor, InboxFilter::All, None, 25)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET visibility_status='active',access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);
    // NEwBOO excludes pending_deletion from anonymous discovery but keeps
    // a private direct-view reference for an eligible user until hard deletion.
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);
    assert_eq!(
        inbox
            .list(actor, InboxFilter::All, None, 25)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        repo.set_reply_read_state(actor, second, false).await,
        ReadStateResult::Updated
    );
    // Lifecycle state never overrides a revoked age entitlement.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    assert!(inbox
        .list(actor, InboxFilter::All, None, 25)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query(
        "UPDATE books SET lifecycle_status='in_progress',access_level='basic' WHERE id=$1",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(repo.received_replies(actor, 10).await.unwrap().len(), 2);

    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    assert!(repo.received_replies(actor, 10).await.unwrap().is_empty());
    assert!(inbox
        .list(actor, InboxFilter::All, None, 25)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='administrator'")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1 AND level='pro_adult'")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
}
