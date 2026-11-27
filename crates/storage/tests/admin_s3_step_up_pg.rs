//! AUTH-06b: real PostgreSQL proof consumption; no external S3 or real keys.
#![cfg(feature = "pg")]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_contracts::auth::S3_CONNECTION_STEP_UP_SCOPE;
use tor_storage::repositories::admin_s3_profiles::{
    connection_intent_digest, AdminS3Error, PgAdminS3Profiles,
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and synthetic encryption key"]
async fn auth_06_s3_connection_proof_consumed_atomically_and_only_once() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    assert_eq!(
        std::env::var("TOR_S3_ADMIN_MASTER_KEY").as_deref(),
        Ok("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
    );
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let profiles = PgAdminS3Profiles::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S3 step-up fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("s3-step-up-{nonce}@example.invalid"))
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
    let digest: [u8; 32] = Sha256::digest(format!("s3-session-{nonce}").as_bytes()).into();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(format!("s3-proof-{nonce}").as_bytes()));
    let proof_hash = Sha256::digest(token.as_bytes());
    let bucket = "ci-disposable";
    let access = "CI_TEST_ACCESS_0000";
    let secret = "CI_TEST_SECRET_000000000000";
    let intent = connection_intent_digest(bucket, access, secret);
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+INTERVAL '1 hour')",
    )
    .bind(actor)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();

    async fn audit_count(pool: &sqlx::PgPool, actor: i64) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM admin_s3_profile_audit WHERE performed_by_user_id=$1",
        )
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    assert_eq!(
        profiles
            .save_verified(&digest, &token, bucket, access, secret)
            .await,
        Err(AdminS3Error::StepUpRequired)
    );
    assert_eq!(audit_count(&pool, actor).await, 0);
    // Explicit isolated fixture represents a proof minted by A. A's real
    // issuer, password checks and scope preservation are tested separately.
    sqlx::query(
        "UPDATE sessions SET step_up_proof_hash=$2,step_up_scope=$3,
           step_up_resource_id=1,step_up_revision=1,step_up_payload_digest=$4,
           step_up_issued_at=now(),step_up_expires_at=now()+INTERVAL '5 minutes'
         WHERE token_hash=$1",
    )
    .bind(digest.as_slice())
    .bind(proof_hash.as_slice())
    .bind(S3_CONNECTION_STEP_UP_SCOPE)
    .bind(intent.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        profiles
            .save_verified(&digest, &token, "different-bucket", access, secret)
            .await,
        Err(AdminS3Error::StepUpRequired)
    );
    assert_eq!(
        profiles
            .save_verified(&digest, &token, bucket, access, "another-secret-0000000000")
            .await,
        Err(AdminS3Error::StepUpRequired)
    );
    assert_eq!(audit_count(&pool, actor).await, 0);
    profiles
        .save_verified(&digest, &token, bucket, access, secret)
        .await
        .unwrap();
    assert_eq!(audit_count(&pool, actor).await, 1);
    let saved = profiles.overview().await.unwrap().unwrap();
    assert_eq!(saved.bucket, bucket);
    assert!(!saved.active);
    assert_eq!(
        profiles
            .save_verified(&digest, &token, bucket, access, secret)
            .await,
        Err(AdminS3Error::StepUpRequired)
    );
    sqlx::query(
        "UPDATE sessions SET step_up_used_at=NULL,
           step_up_issued_at=now()-INTERVAL '6 minutes',
           step_up_expires_at=now()-INTERVAL '1 minute'
         WHERE token_hash=$1",
    )
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        profiles
            .save_verified(&digest, &token, bucket, access, secret)
            .await,
        Err(AdminS3Error::StepUpRequired)
    );
    sqlx::query(
        "UPDATE sessions SET step_up_issued_at=now(),
           step_up_expires_at=now()+INTERVAL '5 minutes'
         WHERE token_hash=$1",
    )
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        profiles
            .save_verified(&digest, &token, bucket, access, secret)
            .await,
        Err(AdminS3Error::Forbidden)
    );
    assert_eq!(audit_count(&pool, actor).await, 1);
}
