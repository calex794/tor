//! S018: browser form boundaries with disposable PG and no real email send.
use super::*;
use tor_reading::repositories::{PgEmailNotificationConsent, PgNotificationPreferences};

async fn consent_post(
    app: &Router,
    path: &str,
    cookies: &str,
    origin: &str,
    body: &str,
) -> StatusCode {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn s018_email_proof_ui_never_autosends_and_requires_actor_csrf_and_consent() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated DB");
    assert!(url.ends_with("/tor_ci"), "no production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ConsentVerification2026").unwrap();
    let alice_token = generate_session_token();
    let bob_token = generate_session_token();
    let csrf = generate_session_token();
    let alice = accounts
        .register_with_session(
            &format!("s018-consent-a-{nonce}@example.invalid"),
            "Consent A",
            &password,
            &session_token_digest(&alice_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let bob = accounts
        .register_with_session(
            &format!("s018-consent-b-{nonce}@example.invalid"),
            "Consent B",
            &password,
            &session_token_digest(&bob_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let one = format!("tor-dev-session={alice_token}; tor-dev-csrf={csrf}");
    let two = format!("tor-dev-session={bob_token}; tor-dev-csrf={csrf}");
    let consent = PgEmailNotificationConsent::new(pool.clone());
    let prefs = PgNotificationPreferences::new(pool.clone());

    let page = c17_settings_page(&app, &one).await;
    assert!(page.contains("Подтверждение email пока не подключено"));
    assert!(!page.contains("Подтвердить email и согласие"));
    assert_eq!(
        consent_post(
            &app,
            "/me/notification-email/request",
            &one,
            ORIGIN,
            &format!("csrf_token={csrf}")
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
        "no proof MTA configured"
    );
    assert_eq!(
        consent_post(
            &app,
            "/me/notification-email/revoke",
            &one,
            "https://wrong.invalid",
            &format!("csrf_token={csrf}")
        )
        .await,
        StatusCode::FORBIDDEN
    );
    let code = generate_session_token();
    let digest = session_token_digest(&code);
    assert!(consent.issue_proof(alice, &digest).await.unwrap());

    let confirm = format!("csrf_token={csrf}&proof_code={code}&email_opt_in=on");
    assert_eq!(
        consent_post(&app, "/me/notification-email/confirm", &two, ORIGIN, &confirm).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        consent_post(
            &app,
            "/me/notification-email/confirm",
            &one,
            "https://wrong.invalid",
            &confirm
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        consent_post(
            &app,
            "/me/notification-email/confirm",
            &one,
            ORIGIN,
            &format!("csrf_token={csrf}&proof_code={code}")
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "proof alone is not opt-in"
    );
    assert_eq!(
        consent_post(&app, "/me/notification-email/confirm", &one, ORIGIN, &confirm).await,
        StatusCode::SEE_OTHER
    );
    assert!(!consent.email_eligible(alice).await.unwrap(), "cadence off");
    assert!(consent.opted_in(alice).await.unwrap());
    assert!(!consent.opted_in(bob).await.unwrap());
    assert!(c17_settings_page(&app, &one).await.contains("Отозвать согласие"));
    prefs
        .set_delivery_preference(alice, tor_reading::repositories::DeliveryMode::Immediate)
        .await
        .unwrap();
    assert!(consent.email_eligible(alice).await.unwrap());
    assert_eq!(
        consent_post(&app, "/me/notification-email/confirm", &one, ORIGIN, &confirm).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "replay rejected"
    );
    assert_eq!(
        consent_post(
            &app,
            "/me/notification-email/revoke",
            &one,
            ORIGIN,
            &format!("csrf_token={csrf}")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(!consent.opted_in(alice).await.unwrap());
    assert!(!consent.email_eligible(alice).await.unwrap());
    pool.close().await;
}
