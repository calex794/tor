//! S039: real A-issued password proof to internal D posting, disposable PG only.
#![forbid(unsafe_code)]
use std::{num::NonZeroU32, process::Command, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{
        generate_session_token, hash_password, session_token_digest,
        verify_password_for_step_up,
    },
};
use tor_books::repositories::PgCatalog;
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::{
        creator_finance::PgCreatorFinance,
        creator_payout_posting::{
            payout_step_up_payload_digest, PgCreatorPayoutPosting, PayoutPostError,
        },
    },
};

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use retained DB");
    let result = Command::new("psql")
        .env("PGOPTIONS", "-c statement_timeout=15000 -c lock_timeout=5000")
        .args(["-X","-A","-t","-q","-v","ON_ERROR_STOP=1","--dbname",url,"--command",sql])
        .output().expect("psql required for PostgreSQL acceptance");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and no HTTP"]
async fn s039_password_step_up_is_session_bound_and_posted_only_once() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(url.ends_with("/tor_ci"), "never use retained DB");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let admin_pass = "S039PayoutAdminPassword2026";
    let admin_hash = hash_password(admin_pass).unwrap();
    let admin_token = generate_session_token();
    let admin_digest = session_token_digest(&admin_token);
    let admin = accounts.register_with_session(
        &format!("s039-stepup-admin-{stamp}@example.invalid"),
        "S039 payout administrator", &admin_hash, &admin_digest,
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    let owner_token = generate_session_token();
    let owner_hash = hash_password("S039PayoutAuthorPassword2026").unwrap();
    let owner = accounts.register_with_session(
        &format!("s039-stepup-owner-{stamp}@example.invalid"),
        "S039 payout author", &owner_hash,
        &session_token_digest(&owner_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s039:stepup:{stamp}','ru','S039 trusted approval','public',{owner})"
    ));
    pg(&url, &format!(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES({owner},1000,'RUB')"
    ));
    pg(&url, &format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({admin},'administrator',true)"
    ));
    let id = PgCreatorFinance::new(pool.clone()).reserve_once(
        owner,600,&IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap(),
    ).await.unwrap().id;
    let reason = "Issue and post a bounded payout";
    let intended = payout_step_up_payload_digest(admin,id,600,reason);
    let password_proof = verify_password_for_step_up(admin_pass,&admin_hash).unwrap();
    let token = accounts.issue_administrator_payout_step_up(
        admin,&admin_digest,&password_proof,id,600,&intended,
    ).await.unwrap();
    assert_eq!(token.len(),43);
    let posting = PgCreatorPayoutPosting::new(pool.clone());
    let receipt = posting.approve_once(
        admin,&admin_digest,&token,id,600,reason,
    ).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(pg(&url,&format!(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}"
    )),"400");
    assert_eq!(pg(&url,&format!(
        "SELECT state FROM finance_creator_payout_requests WHERE id={id}"
    )),"posted");
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM finance_creator_payout_postings WHERE payout_request_id={id}"
    )),"1");
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM reader_wallet_entries e
         JOIN finance_creator_payout_postings p ON p.wallet_entry_id=e.id
         WHERE p.payout_request_id={id} AND e.delta_minor=-600
           AND e.kind='creator_payout_debit' AND e.user_id={owner}"
    )),"1");
    assert_eq!(posting.approve_once(
        admin,&admin_digest,&token,id,600,reason,
    ).await.unwrap().wallet_entry_id,receipt.wallet_entry_id);
    assert_eq!(
        posting.approve_once(admin,&admin_digest,&token,id,600,"Changed").await,
        Err(PayoutPostError::Conflict),
    );
    pg(&url,&format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin} AND role='administrator'"
    ));
    assert_eq!(
        posting.approve_once(admin,&admin_digest,&token,id,600,reason).await,
        Err(PayoutPostError::Forbidden),
    );
    pg(&url,&format!(
        "UPDATE sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp())
         WHERE user_id={admin}"
    ));
    pool.close().await;
}
