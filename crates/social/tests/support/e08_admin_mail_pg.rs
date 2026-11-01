//! E-08 admin read proof against disposable /tor_ci only. No web route.
use super::*;
use tor_social::repositories::admin_mail::PgAdminMail;

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_admin_mail_requires_live_role_session_and_durable_access_audit() {
    let (pool, users) = fixture().await;
    let (admin, sender, recipient) = (users[0], users[1], users[2]);
    let mut admin_digest = [0_u8; 32];
    admin_digest[..8].copy_from_slice(&admin.to_be_bytes());
    let mut user_digest = [0_u8; 32];
    user_digest[..8].copy_from_slice(&sender.to_be_bytes());
    for (user_id, digest) in [(admin, &admin_digest), (sender, &user_digest)] {
        sqlx::query(
            "INSERT INTO sessions(token_hash,user_id,expires_at)
             VALUES($1,$2,now()+interval '1 hour')",
        )
        .bind(digest.as_slice())
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();

    let direct = MailService::new(PgMail::new(pool.clone()));
    let conversation_id = direct.start(sender, recipient).await.unwrap();
    let first = direct
        .send(sender, conversation_id, "Strictly private", &key(901))
        .await
        .unwrap()
        .id;
    let second = direct
        .send(recipient, conversation_id, "Reply secret", &key(902))
        .await
        .unwrap()
        .id;

    let store = PgAdminMail::new(pool.clone());
    assert_eq!(
        store.users(&user_digest, "", None, 10).await,
        Err(MailError::NotFound)
    );
    assert_eq!(
        store.mailbox(&user_digest, sender, None, 10).await,
        Err(MailError::NotFound)
    );
    assert_eq!(
        store
            .messages(&user_digest, conversation_id, None, 10)
            .await,
        Err(MailError::NotFound)
    );
    assert_eq!(
        store
            .messages(&[42_u8; 32], conversation_id, None, 10)
            .await,
        Err(MailError::NotFound)
    );
    assert_eq!(
        store
            .messages(&admin_digest, conversation_id, None, 0)
            .await,
        Err(MailError::Invalid)
    );

    let all_users = store
        .users(&admin_digest, "MAIL FIX", None, 50)
        .await
        .unwrap();
    assert!(all_users.iter().any(|row| row.user_id == sender));
    assert!(all_users.iter().any(|row| row.user_id == recipient));
    assert_eq!(
        all_users
            .iter()
            .find(|row| row.user_id == admin)
            .unwrap()
            .conversation_count,
        0
    );
    assert_eq!(
        all_users
            .iter()
            .find(|row| row.user_id == sender)
            .unwrap()
            .conversation_count,
        1
    );
    let inbox = store
        .mailbox(&admin_digest, sender, None, 50)
        .await
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].id, conversation_id);
    assert_eq!(inbox[0].message_count, 2);

    let full = store
        .messages(&admin_digest, conversation_id, None, 50)
        .await
        .unwrap();
    assert_eq!(full.len(), 2);
    assert_eq!(full[0].id, first);
    assert_eq!(full[0].body, "Strictly private");
    assert_eq!(full[1].id, second);
    let older = store
        .messages(&admin_digest, conversation_id, Some(second), 1)
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, first);

    // Four successful scoped operations are recorded before returning data.
    let events: Vec<(String, Option<i64>, Option<i64>, i32)> = sqlx::query_as(
        "SELECT action,target_user_id,conversation_id,result_count
         FROM social_mail_admin_access_audit
         WHERE admin_user_id=$1 ORDER BY id",
    )
    .bind(admin)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].0, "search_mail_users");
    assert_eq!(
        events[1],
        ("list_user_mailbox".to_owned(), Some(sender), None, 1)
    );
    assert_eq!(
        events[2],
        (
            "read_direct_conversation".to_owned(),
            None,
            Some(conversation_id),
            2
        )
    );
    assert_eq!(events[3].3, 1);
    assert!(sqlx::query(
        "UPDATE social_mail_admin_access_audit SET result_count=0
             WHERE admin_user_id=$1"
    )
    .bind(admin)
    .execute(&pool)
    .await
    .is_err());

    // Revoke the authority while leaving the login session active.
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .messages(&admin_digest, conversation_id, None, 50)
            .await,
        Err(MailError::NotFound)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=TRUE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(admin_digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.users(&admin_digest, "", None, 10).await,
        Err(MailError::NotFound)
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_mail_admin_access_audit WHERE admin_user_id=$1",
    )
    .bind(admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 4,
        "denied reads must not become successful audits"
    );

    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='administrator'")
        .bind(admin)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}


/// Admin-mail audit remains immutable to app connections. Retention runs
/// only on ephemeral tor_ci as the separate tor_migrator role and never via HTTP.
#[tokio::test]
#[ignore = "requires disposable /tor_ci and MIGRATOR_URL"]
async fn e_08_admin_mail_retention_requires_migrator_and_preserves_recent_history() {
    let (app, users) = fixture().await;
    let actor = users[0];
    let old: i64 = sqlx::query_scalar(
        "INSERT INTO social_mail_admin_access_audit
         (admin_user_id,action,result_count,created_at)
         VALUES($1,'search_mail_users',0,now()-INTERVAL '61 days')
         RETURNING id",
    )
    .bind(actor)
    .fetch_one(&app)
    .await
    .unwrap();
    let recent: i64 = sqlx::query_scalar(
        "INSERT INTO social_mail_admin_access_audit
         (admin_user_id,action,result_count)
         VALUES($1,'search_mail_users',0)
         RETURNING id",
    )
    .bind(actor)
    .fetch_one(&app)
    .await
    .unwrap();

    // Ordinary connections cannot mutate history or invoke retention.
    assert!(
        sqlx::query("UPDATE social_mail_admin_access_audit SET result_count=1 WHERE id=$1")
            .bind(old)
            .execute(&app)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM social_mail_admin_access_audit WHERE id=$1")
            .bind(old)
            .execute(&app)
            .await
            .is_err()
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT public.social_mail_admin_audit_purge_before(now()-INTERVAL '30 days')"
        )
        .fetch_one(&app)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("SELECT id FROM social_mail_admin_audit_retention_runs")
            .fetch_all(&app)
            .await
            .is_err()
    );

    let url = std::env::var("MIGRATOR_URL").unwrap();
    assert!(url.contains("127.0.0.1:5432/tor_ci"));
    let migrator = PgPool::connect(&url).await.unwrap();
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert_eq!(role, "tor_migrator");
    assert!(
        sqlx::query("DELETE FROM social_mail_admin_access_audit WHERE id=$1")
            .bind(old)
            .execute(&migrator)
            .await
            .is_err(),
        "direct DELETE still has the immutable audit trigger"
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT public.social_mail_admin_audit_purge_before(NULL)"
        )
        .fetch_one(&migrator)
        .await
        .is_err()
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT public.social_mail_admin_audit_purge_before(now()+INTERVAL '1 day')"
        )
        .fetch_one(&migrator)
        .await
        .is_err()
    );

    let removed: i64 = sqlx::query_scalar(
        "SELECT public.social_mail_admin_audit_purge_before(now()-INTERVAL '30 days')",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert!(removed >= 1);
    let remaining_old: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_mail_admin_access_audit WHERE id=$1",
    )
    .bind(old)
    .fetch_one(&app)
    .await
    .unwrap();
    let remaining_recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_mail_admin_access_audit WHERE id=$1",
    )
    .bind(recent)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!((remaining_old, remaining_recent), (0, 1));

    let receipt: (i64, bool) = sqlx::query_as(
        "SELECT removed_count,cutoff_before<executed_at
         FROM social_mail_admin_audit_retention_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(receipt, (removed, true));
    assert!(
        sqlx::query("UPDATE social_mail_admin_audit_retention_runs SET removed_count=0")
            .execute(&migrator)
            .await
            .is_err(),
        "the operator may not tamper with retention receipts"
    );
    assert!(
        sqlx::query("DELETE FROM social_mail_admin_audit_retention_runs")
            .execute(&migrator)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM social_mail_admin_access_audit WHERE id=$1")
            .bind(recent)
            .execute(&migrator)
            .await
            .is_err(),
        "purge permission must not persist after the function returns"
    );
    // A second explicit cutoff is idempotent and still records a receipt.
    let second: i64 = sqlx::query_scalar(
        "SELECT public.social_mail_admin_audit_purge_before(now()-INTERVAL '30 days')",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(second, 0);

    migrator.close().await;
    cleanup(&app, &users).await;
}
