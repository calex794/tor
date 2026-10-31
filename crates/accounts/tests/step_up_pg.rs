//! A-owned fresh password proof, restricted to disposable tor_ci.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::{step_up::StepUpIssueError, PgAccounts},
    services::security::{
        hash_password, session_token_digest, verify_password_for_step_up,
    },
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_admin_refund_step_up_binds_live_session_and_password_rotation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never use a real database");
    let pool = PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let first_hash = hash_password("ValidModeratorPassword2026").unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'A-owned test administrator',$2) RETURNING id",
    )
    .bind(format!("step_up_{unique}@example.test"))
    .bind(&first_hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();

    let session_digest = session_token_digest(&format!("a07:step-up:{unique}"));
    accounts
        .insert_session(actor, &session_digest, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let digest: [u8; 32] = Sha256::digest(b"exact refund approval intent").into();
    let verified =
        verify_password_for_step_up("ValidModeratorPassword2026", &first_hash).unwrap();
    let token = accounts
        .issue_administrator_refund_step_up(actor, &session_digest, &verified, 5, 1, &digest)
        .await
        .unwrap();

    let (stored_hash, stored_scope, resource, revision, payload_match, ttl_valid, used): (
        Vec<u8>,
        String,
        i64,
        i64,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT step_up_proof_hash,step_up_scope,step_up_resource_id,
                step_up_revision,(step_up_payload_digest=$2),
                (step_up_expires_at>step_up_issued_at AND
                 step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'),
                step_up_used_at IS NOT NULL
         FROM sessions WHERE token_hash=$1",
    )
    .bind(session_digest.as_slice())
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_hash, session_token_digest(&token).to_vec());
    assert_ne!(stored_hash.as_slice(), token.as_bytes());
    assert_eq!(stored_scope, "finance.refund.approve.v1");
    assert_eq!((resource, revision, payload_match, ttl_valid, used), (5, 1, true, true, false));
    assert_eq!(token.len(), 43);

    assert!(verify_password_for_step_up("wrong password", &first_hash).is_none());
    let rotated = hash_password("RotatedModeratorPassword2026").unwrap();
    sqlx::query("UPDATE users SET password_hash=$1 WHERE id=$2")
        .bind(rotated)
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        accounts
            .issue_administrator_refund_step_up(actor, &session_digest, &verified, 5, 1, &digest)
            .await,
        Err(StepUpIssueError::StepUpRequired)
    );
    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(session_digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let newly_verified =
        verify_password_for_step_up("ValidModeratorPassword2026", &first_hash).unwrap();
    assert_eq!(
        accounts
            .issue_administrator_refund_step_up(
                actor,
                &session_digest,
                &newly_verified,
                5,
                1,
                &digest,
            )
            .await,
        Err(StepUpIssueError::StepUpRequired)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}
