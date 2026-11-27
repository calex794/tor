use super::*;
use std::{ffi::OsString, time::Duration};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};
use tor_mail::{domain::quota::identity::QuotaKeySigner, repositories::PgMailQueue};

struct Environment(Vec<(&'static str, Option<OsString>)>);

impl Environment {
    fn set(values: &[(&'static str, &str)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, value)| {
                let previous = std::env::var_os(key);
                std::env::set_var(key, value);
                (*key, previous)
            })
            .collect();
        Self(saved)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..) {
            if let Some(value) = previous {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

fn app(catalog: &PgCatalog, accounts: &PgAccounts) -> Router {
    tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(catalog.pool().clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    )
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci; no mail sender"]
async fn a_07_reset_queue_http_generic_ack_and_restart_handoff() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let db = accounts.pool();
    let key = "4f".repeat(32);
    let _env = Environment::set(&[
        ("TOR_PASSWORD_RESET_ENABLED", "1"),
        ("TOR_PASSWORD_RESET_QUEUE_ENABLED", "1"),
        ("TOR_MAIL_QUOTA_KEY", ""),
        ("TOR_RESET_SENDMAIL", "/bin/false"),
        ("TOR_RESET_MAIL_FROM", "noreply@example.invalid"),
    ]);
    // Invalid queue configuration must not fall through to the local MTA.
    let unavailable = app(&catalog, &accounts);
    let disabled = send(
        &unavailable,
        request("GET", "/forgot-password", "", "", ORIGIN),
    )
    .await;
    assert!(html(disabled).await.contains("временно недоступно"));
    assert!(tor_web::start_password_reset_handoff(Some(accounts.clone())).is_none());

    std::env::set_var("TOR_MAIL_QUOTA_KEY", &key);
    let app = app(&catalog, &accounts);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("reset-http-{nonce}@example.invalid");
    let user = accounts
        .insert_user(&email, "Reset fixture", "$argon2id$fixture")
        .await
        .unwrap();
    let old_proof = session_token_digest(&format!("existing-reset-http-{nonce}"));
    assert!(accounts
        .issue_password_reset(user, "$argon2id$fixture", &old_proof)
        .await
        .unwrap());

    let page = send(&app, request("GET", "/forgot-password", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let cookies = cookie(&page, "tor-dev-csrf=");
    let markup = html(page).await;
    let token = csrf(&markup);
    let form = format!("email={}&csrf_token={token}", email.replace('@', "%40"));
    let bad_origin = send(
        &app,
        request(
            "POST",
            "/forgot-password",
            &form,
            &cookies,
            "https://evil.invalid",
        ),
    )
    .await;
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    let missing_csrf = send(
        &app,
        request("POST", "/forgot-password", &form, "", ORIGIN),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_password_reset_mail_requests WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(count, 0);

    let mut known = from_peer(
        request("POST", "/forgot-password", &form, &cookies, ORIGIN),
        [192, 0, 2, 7],
    );
    known
        .headers_mut()
        .insert("x-real-ip", HeaderValue::from_static("203.0.113.99"));
    let known = send(&app, known).await;
    let unknown = format!("email=absent-{nonce}%40example.invalid&csrf_token={token}");
    let unknown = send(
        &app,
        from_peer(
            request("POST", "/forgot-password", &unknown, &cookies, ORIGIN),
            [192, 0, 2, 7],
        ),
    )
    .await;
    assert_eq!(known.status(), StatusCode::OK);
    assert_eq!(unknown.status(), StatusCode::OK);
    assert_eq!(known.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        known.headers()[header::CACHE_CONTROL],
        unknown.headers()[header::CACHE_CONTROL]
    );
    assert_eq!(html(known).await, html(unknown).await);

    let event_id = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM account_password_reset_mail_requests WHERE user_id=$1",
            )
            .bind(user)
            .fetch_optional(db)
            .await
            .unwrap();
            if let Some(id) = id {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("bounded ingress must commit a source");
    let intent = MailIntent {
        purpose: MailPurpose::PasswordReset,
        event_id: PositiveId::new(event_id).unwrap(),
        recipient: UserId::new(user).unwrap(),
    };
    let (evidence, address) = accounts
        .resolve_password_reset_mail_request(intent)
        .await
        .unwrap()
        .unwrap();
    let signer = QuotaKeySigner::from_hex(&key).unwrap();
    let expected = signer
        .keys(&email, Some("192.0.2.7".parse().unwrap()))
        .unwrap()
        .password_reset_source();
    assert!(evidence.source == expected);
    assert_eq!(address, email);

    // Reproduce a committed enqueue with a lost source acknowledgement.
    let queue = PgMailQueue::new(db.clone());
    assert!(queue.enqueue(intent).await.unwrap());
    let worker = tor_web::start_password_reset_handoff(Some(accounts.clone())).unwrap();
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let forwarded: bool = sqlx::query_scalar(
                "SELECT forwarded_at IS NOT NULL FROM account_password_reset_mail_requests \
                 WHERE id=$1",
            )
            .bind(event_id)
            .fetch_one(db)
            .await
            .unwrap();
            if forwarded {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("handoff must recover a lost acknowledgement");
    worker.abort();
    let _ = worker.await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_outbox WHERE purpose='password_reset' \
         AND event_id=$1 AND recipient_user_id=$2 AND status='pending'",
    )
    .bind(event_id)
    .bind(user)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let proofs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM password_reset_tokens WHERE user_id=$1")
            .bind(user)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(proofs, 1);
    assert!(accounts
        .live_reset_password_hash(&old_proof)
        .await
        .unwrap()
        .is_some());
}
