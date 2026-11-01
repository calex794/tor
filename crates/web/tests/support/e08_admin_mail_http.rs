//! E-08 administrator mail route ACL and immutable audit, disposable tor_ci only.
use super::{call, pg, text, ORIGIN};
use axum::http::{header, StatusCode};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_social::{repositories::mail::PgMail, services::MailService};

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_admin_mail_html_requires_live_role_and_audits() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let hash = hash_password("AdminMailTestingPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    for i in 0..3 {
        let session = generate_session_token();
        let csrf = generate_session_token();
        let id = accounts.register_with_session(
            &format!("admin-mail-http-{nonce}-{i}@example.invalid"),
            &format!("Admin mail reader {i}"),
            &hash,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        ).await.unwrap();
        users.push(id);
        cookies.push(format!("tor-dev-session={session}; tor-dev-csrf={csrf}"));
    }
    let mail = MailService::new(PgMail::new(pool.clone()));
    let id = mail.start(users[1], users[2]).await.unwrap();
    mail.send(users[1], id, "Strictly <img src=x onerror=alert(1)>", &format!("{:043}", 880))
        .await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog, PgReader::new(pool), accounts, false, ORIGIN.to_owned()
    );
    let mailbox = format!("/admin/mail/users/{}", users[1]);
    let detail = format!("/admin/mail/{id}");
    for path in ["/admin/mail", &mailbox, &detail] {
        assert_eq!(call(&app, "GET", path, "", "", &[]).await.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, "GET", path, &cookies[1], "", &[]).await.status(), StatusCode::NOT_FOUND);
    }
    pg(&url, &format!("INSERT INTO account_roles(user_id,role,is_active) VALUES({},'administrator',TRUE)", users[0])).await;
    let response = call(&app, "GET", "/admin/mail?q=Admin", &cookies[0], "", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers()["content-security-policy"].to_str().unwrap().contains("script-src 'none'"));
    let html = text(response).await;
    assert!(html.contains("Admin mail reader 1"));
    assert!(html.contains(&mailbox));
    assert!(!html.contains("example.invalid"));
    let response = call(&app, "GET", &mailbox, &cookies[0], "", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains(&detail));
    let response = call(&app, "GET", &detail, &cookies[0], "", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Strictly &lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("<img src=x onerror=alert(1)>"));
    assert_eq!(call(&app, "GET", "/admin/mail?before=0", &cookies[0], "", &[]).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let audit = format!("SELECT count(*) FROM social_mail_admin_access_audit WHERE admin_user_id={}", users[0]);
    assert_eq!(pg(&url, &audit).await, "3");
    pg(&url, &format!("UPDATE account_roles SET is_active=FALSE WHERE user_id={} AND role='administrator'", users[0])).await;
    assert_eq!(call(&app, "GET", &detail, &cookies[0], "", &[]).await.status(), StatusCode::NOT_FOUND);
    pg(&url, &format!("UPDATE account_roles SET is_active=TRUE WHERE user_id={} AND role='administrator'", users[0])).await;
    pg(&url, &format!("UPDATE sessions SET revoked_at=now() WHERE user_id={}", users[0])).await;
    assert_eq!(call(&app, "GET", &detail, &cookies[0], "", &[]).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(pg(&url, &audit).await, "3");
}
