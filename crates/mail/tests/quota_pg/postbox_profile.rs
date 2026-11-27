use super::*;
use sha2::{Digest, Sha256};
use tor_contracts::auth::POSTBOX_PROFILE_STEP_UP_SCOPE;
use tor_mail::{
    domain::postbox_profile::{ProfileIdentity, ProfileMasterKey, ProfileSecrets},
    repositories::postbox_profile::{PgPostboxProfile, ProfileError},
};

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_profile_sealed_candidate_requires_live_role_proof_and_revision() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let store = PgPostboxProfile::new(pool.clone());
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Postbox profile fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("postbox-profile-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(nonce.to_be_bytes()).into();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 day')",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        store.admin_view(&digest).await,
        Err(ProfileError::Forbidden)
    ));
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',true)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let revision = store.admin_view(&digest).await.unwrap().revision;
    let master = ProfileMasterKey::from_base64(1, &STANDARD.encode([7; 32])).unwrap();
    let secrets =
        ProfileSecrets::new("ci-profile-access".into(), "ci-profile-secret".into()).unwrap();
    let identity =
        ProfileIdentity::new("ci-identity".into(), "sender@example.invalid".into()).unwrap();
    let candidate = master
        .seal(identity.clone(), revision + 1, &secrets)
        .unwrap();
    let other = master.seal(identity, revision + 1, &secrets).unwrap();
    let proof = "x".repeat(43);
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::StepUpRequired)
    );
    let proof_hash: [u8; 32] = Sha256::digest(proof.as_bytes()).into();
    sqlx::query(
        "UPDATE sessions SET step_up_proof_hash=$1,step_up_scope=$2,
         step_up_resource_id=1,step_up_revision=$3,step_up_payload_digest=$4,
         step_up_issued_at=clock_timestamp(),step_up_expires_at=clock_timestamp()+interval '2 minutes',
         step_up_used_at=NULL WHERE token_hash=$5",
    )
    .bind(proof_hash.as_slice())
    .bind(POSTBOX_PROFILE_STEP_UP_SCOPE)
    .bind(revision)
    .bind(candidate.intent_digest().as_slice())
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &other)
            .await,
        Err(ProfileError::StepUpRequired)
    );
    sqlx::query("UPDATE mail_cost_policy SET enabled=true WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::Conflict)
    );
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    store
        .admin_save_candidate(&digest, &proof, revision, &candidate)
        .await
        .unwrap();
    let restarted = PgPostboxProfile::new(PgPool::connect(&url).await.unwrap());
    let view = restarted.admin_view(&digest).await.unwrap();
    assert_eq!(view.revision, revision + 1);
    assert_eq!(view.key_version, Some(1));
    let loaded = restarted
        .admin_load_candidate(&digest, revision + 1)
        .await
        .unwrap();
    let recovered = master.open(&loaded).unwrap();
    assert_eq!(recovered.access(), "ci-profile-access");
    assert_eq!(recovered.secret(), "ci-profile-secret");
    assert_eq!(
        restarted
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::Conflict)
    );
    let used: bool =
        sqlx::query_scalar("SELECT step_up_used_at IS NOT NULL FROM sessions WHERE token_hash=$1")
            .bind(digest.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(used);
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM mail_postbox_profile WHERE singleton")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&ciphertext).contains("ci-profile-secret"));
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_postbox_profile_audit WHERE actor_user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 1);
    sqlx::query("UPDATE account_roles SET is_active=false WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        restarted.admin_load_candidate(&digest, revision + 1).await,
        Err(ProfileError::Forbidden)
    ));
    // This is a disposable database; keep revision/audit monotonic.
    sqlx::query(
        "UPDATE mail_postbox_profile SET identity_id=NULL,sender=NULL,key_version=NULL,
         nonce=NULL,ciphertext=NULL,saved_at=NULL WHERE singleton",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
}
