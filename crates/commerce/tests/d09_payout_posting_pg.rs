//! S039/MONEY-03: disposable PostgreSQL trusted internal payout posting.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::{
        creator_finance::PgCreatorFinance,
        creator_payout_posting::{
            payout_step_up_payload_digest, PayoutPostError, PgCreatorPayoutPosting,
        },
    },
};

struct Setup {
    pool: PgPool,
    owner: i64,
    admin: i64,
    request: i64,
    second: i64,
    session: [u8; 32],
}
const TOKEN: &str = "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP";

async fn setup(tag: &str) -> Setup {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci required");
    assert!(url.ends_with("/tor_ci"), "never test with retained DB");
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Payout owner','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-post-owner-{tag}-{stamp}@example.invalid"))
    .fetch_one(&pool).await.unwrap();
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Payout admin','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-post-admin-{tag}-{stamp}@example.invalid"))
    .fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','S039 post fixture','public',$2)",
    )
    .bind(format!("ci:s039:post:{tag}:{stamp}"))
    .bind(owner).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(owner).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)",
    )
    .bind(admin).execute(&pool).await.unwrap();
    let mut session = [49_u8; 32];
    session[..8].copy_from_slice(&admin.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 hour')",
    )
    .bind(admin).bind(session.as_slice()).execute(&pool).await.unwrap();
    let finance = PgCreatorFinance::new(pool.clone());
    let request = finance.reserve_once(
        owner,600,&IdempotencyKey::parse(&format!("{}8","P".repeat(42))).unwrap(),
    ).await.unwrap().id;
    let second = finance.reserve_once(
        owner,300,&IdempotencyKey::parse(&format!("{}8","Q".repeat(42))).unwrap(),
    ).await.unwrap().id;
    Setup { pool, owner, admin, request, second, session }
}

impl Setup {
    async fn proof(&self, reason: &str) {
        let digest = payout_step_up_payload_digest(
            self.admin, self.request, 600, reason,
        );
        let hash = Sha256::digest(TOKEN.as_bytes());
        sqlx::query(
            "UPDATE sessions SET step_up_proof_hash=$2,
             step_up_scope='finance.payout.post.v1',
             step_up_resource_id=$3,step_up_revision=600,
             step_up_payload_digest=$4,
             step_up_issued_at=clock_timestamp()-interval '1 second',
             step_up_expires_at=clock_timestamp()+interval '5 minutes',
             step_up_used_at=NULL WHERE token_hash=$1",
        )
        .bind(self.session.as_slice()).bind(hash.as_slice())
        .bind(self.request).bind(digest.as_slice())
        .execute(&self.pool).await.unwrap();
    }

    async fn finish(self) {
        sqlx::query(
            "UPDATE account_roles SET is_active=false
             WHERE user_id=$1 AND role='administrator'",
        )
        .bind(self.admin).execute(&self.pool).await.unwrap();
        sqlx::query(
            "UPDATE sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp())
             WHERE token_hash=$1",
        )
        .bind(self.session.as_slice()).execute(&self.pool).await.unwrap();
        self.pool.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_post_exactly_once_with_other_reservation_protected() {
    let f = setup("once").await;
    let repo = PgCreatorPayoutPosting::new(f.pool.clone());
    f.proof("Approve internally").await;
    let first = repo.approve_once(
        f.admin,&f.session,TOKEN,f.request,600,"Approve internally",
    ).await.unwrap();
    assert!(!first.replayed);
    let again = repo.approve_once(
        f.admin,&f.session,TOKEN,f.request,600,"Approve internally",
    ).await.unwrap();
    assert!(again.replayed);
    assert_eq!(again.wallet_entry_id, first.wallet_entry_id);
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Changed").await,
        Err(PayoutPostError::Conflict)
    );
    let row: (String,Option<i64>,Option<String>) = sqlx::query_as(
        "SELECT state,decision_actor_id,decision_reason
         FROM finance_creator_payout_requests WHERE id=$1",
    ).bind(f.request).fetch_one(&f.pool).await.unwrap();
    assert_eq!(row,("posted".into(),Some(f.admin),Some("Approve internally".into())));
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    ).bind(f.owner).fetch_one(&f.pool).await.unwrap();
    assert_eq!(balance,400);
    let entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_wallet_entries
         WHERE user_id=$1 AND kind='creator_payout_debit'",
    ).bind(f.owner).fetch_one(&f.pool).await.unwrap();
    assert_eq!(entries,1);
    let reserved = PgCreatorFinance::new(f.pool.clone())
        .totals(f.owner).await.unwrap().reserved_minor;
    assert_eq!(reserved,300);
    let second_state: String = sqlx::query_scalar(
        "SELECT state FROM finance_creator_payout_requests WHERE id=$1",
    ).bind(f.second).fetch_one(&f.pool).await.unwrap();
    assert_eq!(second_state,"pending");
    let used: bool = sqlx::query_scalar(
        "SELECT step_up_used_at IS NOT NULL FROM sessions WHERE token_hash=$1",
    ).bind(f.session.as_slice()).fetch_one(&f.pool).await.unwrap();
    assert!(used);
    assert!(sqlx::query(
        "UPDATE finance_creator_payout_postings SET amount_minor=1
         WHERE payout_request_id=$1",
    ).bind(f.request).execute(&f.pool).await.is_err());
    assert!(sqlx::query(
        "UPDATE finance_creator_payout_requests SET state='rejected' WHERE id=$1",
    ).bind(f.request).execute(&f.pool).await.is_err());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_post_denies_missing_proof_wrong_scope_and_revoked_role() {
    let f = setup("deny").await;
    let repo = PgCreatorPayoutPosting::new(f.pool.clone());
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve").await,
        Err(PayoutPostError::StepUpRequired)
    );
    f.proof("Approve").await;
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Changed").await,
        Err(PayoutPostError::StepUpRequired)
    );
    sqlx::query(
        "UPDATE sessions SET step_up_scope='finance.refund.approve.v1'
         WHERE token_hash=$1",
    ).bind(f.session.as_slice()).execute(&f.pool).await.unwrap();
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve").await,
        Err(PayoutPostError::StepUpRequired)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=false
         WHERE user_id=$1 AND role='administrator'",
    ).bind(f.admin).execute(&f.pool).await.unwrap();
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve").await,
        Err(PayoutPostError::Forbidden)
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM finance_creator_payout_requests WHERE id=$1",
    ).bind(f.request).fetch_one(&f.pool).await.unwrap();
    assert_eq!(state,"pending");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_post_underfunding_rolls_back_proof_and_terminal_transition() {
    let f = setup("deficit").await;
    let repo = PgCreatorPayoutPosting::new(f.pool.clone());
    f.proof("Approve").await;
    sqlx::query(
        "INSERT INTO finance_creator_payout_requests
         (user_id,amount_minor,intent_key,intent_digest,reservation_minor)
         VALUES($1,900,$2,decode(repeat('f',64),'hex'),900)",
    )
    .bind(f.owner).bind(format!("{}8","X".repeat(42)))
    .execute(&f.pool).await.unwrap();
    assert_eq!(
        repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve").await,
        Err(PayoutPostError::Insufficient)
    );
    let row: (String,i64) = sqlx::query_as(
        "SELECT r.state,w.balance_minor FROM finance_creator_payout_requests r
         JOIN reader_wallet_accounts w ON w.user_id=r.user_id WHERE r.id=$1",
    ).bind(f.request).fetch_one(&f.pool).await.unwrap();
    assert_eq!(row,("pending".into(),1000));
    let unused: bool = sqlx::query_scalar(
        "SELECT step_up_used_at IS NULL FROM sessions WHERE token_hash=$1",
    ).bind(f.session.as_slice()).fetch_one(&f.pool).await.unwrap();
    assert!(unused);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_creator_payout_postings WHERE payout_request_id=$1",
    ).bind(f.request).fetch_one(&f.pool).await.unwrap();
    assert_eq!(count,0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_post_concurrent_retries_have_one_journal_effect() {
    let f = setup("race").await;
    let repo = PgCreatorPayoutPosting::new(f.pool.clone());
    f.proof("Approve").await;
    let (a,b) = tokio::time::timeout(Duration::from_secs(20),async {
        tokio::join!(
            repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve"),
            repo.approve_once(f.admin,&f.session,TOKEN,f.request,600,"Approve"),
        )
    }).await.expect("approval deadlock");
    let (a,b)=(a.unwrap(),b.unwrap());
    assert_eq!(a.replayed as u8 + b.replayed as u8,1);
    assert_eq!(a.wallet_entry_id,b.wallet_entry_id);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_creator_payout_postings WHERE payout_request_id=$1",
    ).bind(f.request).fetch_one(&f.pool).await.unwrap();
    assert_eq!(count,1);
    f.finish().await;
}
