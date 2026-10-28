//! F-09: constraints and PostgreSQL replay checks.
use tor_referrals::domain::{manual_code, valid_code};

#[test]
fn f_09_invalid_manual_code_is_not_silently_truncated() {
    assert!(manual_code(Some("not a code")).is_err());
    assert!(manual_code(Some("valid_123")).unwrap().is_some());
    assert!(!valid_code("x"));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and current migrations"]
async fn f_09_pg_first_capture_and_replay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = tor_referrals::repositories::PgReferrals::new(pool.clone());
    let email = format!("f09-{}@example.invalid", std::process::id());
    let first: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'F09 referrer','test') RETURNING id",
    )
    .bind(email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let code = format!("f09_{first}");
    sqlx::query(
        "INSERT INTO referral_codes(code,owner_user_id,program_type) \
         VALUES($1,$2,'reader_spend')",
    )
    .bind(&code)
    .bind(first)
    .execute(&pool)
    .await
    .unwrap();
    let guest_hash = [51_u8; 32];
    let saved = repo
        .capture(&guest_hash, &code, "direct_ref_link", "/r/test")
        .await
        .unwrap();
    assert_eq!(
        saved,
        tor_referrals::repositories::postgres::CaptureResult::Saved
    );
    let second = repo
        .capture(&guest_hash, &code, "direct_ref_link", "/r/test")
        .await
        .unwrap();
    assert_eq!(
        second,
        tor_referrals::repositories::postgres::CaptureResult::Existing
    );
    let invited: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'F09 invited','test') RETURNING id",
    )
    .bind(format!("f09-invited-{first}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_registration_referral_outbox(user_id,guest_hash) VALUES($1,$2)",
    )
    .bind(invited)
    .bind(guest_hash.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo.finalize_one(invited).await.unwrap());
    assert!(!repo.finalize_one(invited).await.unwrap());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referral_attributions WHERE subject_user_id=$1 AND referrer_user_id=$2",
    )
    .bind(invited)
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}
