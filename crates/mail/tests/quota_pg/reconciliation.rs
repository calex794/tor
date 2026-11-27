use super::*;
use sha2::{Digest, Sha256};
use tor_mail::{
    domain::cost::MonthUsage,
    repositories::cost::{
        admin::{CostAdminError, MonthReconciliation},
        PgMailCost,
    },
};

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_j_reconcile_stopped_current_month_without_refunds() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let cost = PgMailCost::new(pool.clone());
    clear(&pool).await;
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(&pool)
        .await
        .unwrap();

    let (month, previous): (String, String) = sqlx::query_as(
        "SELECT date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date::text, \
         (date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow') \
         -interval '1 month')::date::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mail_cost_months(month_start,baseline_kopecks,rate_per_thousand,reserved_attempts) \
         VALUES($1::date,300,8032,42)",
    )
    .bind(&previous)
    .execute(&pool)
    .await
    .unwrap();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Mail reconciliation fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("mail-reconcile-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(nonce.to_be_bytes()).into();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at) \
         VALUES($1,$2,clock_timestamp()+interval '1 day')",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let mut request = MonthReconciliation {
        usage: MonthUsage {
            month_start: month.clone(),
            free_allowance: 1000,
            baseline_kopecks: 250,
            rate_per_thousand: 8032,
            reserved_attempts: 20,
        },
        evidence_sha256: [7; 32],
    };
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM mail_cost_policy WHERE singleton")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Forbidden)
    );
    sqlx::query("INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',true)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(cost.snapshot().await.unwrap().unwrap().usage.is_none());

    request.usage.month_start = previous.clone();
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Conflict)
    );
    request.usage.month_start = month;
    request.evidence_sha256 = [0; 32];
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Invalid)
    );
    request.evidence_sha256 = [7; 32];
    sqlx::query("UPDATE mail_cost_policy SET enabled=true WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Conflict)
    );
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();

    // A real committed ledger must be included even when the month row is missing.
    seed(&pool, 2, MailPurpose::SupportReply, keys(17, None), 0).await;
    request.usage.reserved_attempts = 0;
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Invalid)
    );
    request.usage.reserved_attempts = 20;
    cost.admin_reconcile_month(&digest, revision, &request)
        .await
        .unwrap();

    let restarted = PgMailCost::new(PgPool::connect(&url).await.unwrap());
    let view = restarted.admin_view(&digest).await.unwrap();
    assert_eq!(view.revision, revision + 1);
    assert!(!view.snapshot.limits.enabled);
    assert_eq!(view.snapshot.usage, Some(request.usage.clone()));
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision, &request).await,
        Err(CostAdminError::Conflict)
    );

    let mut less = request.usage.clone();
    less.reserved_attempts -= 1;
    let more_free = MonthUsage {
        free_allowance: request.usage.free_allowance + 1,
        ..request.usage.clone()
    };
    let lower_baseline = MonthUsage {
        baseline_kopecks: request.usage.baseline_kopecks - 1,
        ..request.usage.clone()
    };
    for usage in [less, more_free, lower_baseline] {
        let unsafe_request = MonthReconciliation {
            usage,
            evidence_sha256: [8; 32],
        };
        assert_eq!(
            cost.admin_reconcile_month(&digest, revision + 1, &unsafe_request)
                .await,
            Err(CostAdminError::Invalid)
        );
    }
    request.usage.reserved_attempts = 25;
    request.usage.free_allowance = 500;
    request.usage.baseline_kopecks = 500;
    request.usage.rate_per_thousand = 9000;
    request.evidence_sha256 = [9; 32];
    let (first, second) = tokio::join!(
        cost.admin_reconcile_month(&digest, revision + 1, &request),
        restarted.admin_reconcile_month(&digest, revision + 1, &request)
    );
    assert!(matches!(
        (first, second),
        (Ok(()), Err(CostAdminError::Conflict)) | (Err(CostAdminError::Conflict), Ok(()))
    ));
    let view = restarted.admin_view(&digest).await.unwrap();
    assert_eq!(view.revision, revision + 2);
    assert!(!view.snapshot.limits.enabled);
    assert_eq!(view.snapshot.usage, Some(request.usage.clone()));

    let history: i64 = sqlx::query_scalar(
        "SELECT reserved_attempts FROM mail_cost_months WHERE month_start=$1::date",
    )
    .bind(&previous)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(history, 42);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_cost_month_reconciliation_audit WHERE actor_user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 2);
    assert!(sqlx::query(
        "UPDATE mail_cost_month_reconciliation_audit SET new_reserved_attempts=0 WHERE actor_user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query("DELETE FROM mail_cost_month_reconciliation_audit WHERE actor_user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .is_err());

    sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        cost.admin_reconcile_month(&digest, revision + 2, &request).await,
        Err(CostAdminError::Forbidden)
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
}
