//! PG18 browser contract: explicit consent and local path erasure.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::num::NonZeroU32;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    cookie: &str,
    origin: &str,
) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
#[ignore = "isolated PG18 and test-only RIL signer required"]
async fn consent_is_default_off_and_local_erasure_is_confirmed() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    assert_eq!(std::env::var("TOR_RIL_SINGLE_INSTANCE").as_deref(), Ok("1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let reader = PgReader::new(catalog.pool().clone());
    let accounts = PgAccounts::from_pool(catalog.pool());
    let user = accounts
        .insert_user(
            "ril-opt-in-ci@example.invalid",
            "RIL Test",
            &hash_password("unused-test-passphrase").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 0)
    );
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    accounts
        .insert_session(user, &digest, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let session = format!("tor-dev-session={token}");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );

    let first = send(&app, "GET", "/me", "", &session, ORIGIN).await;
    assert_eq!(first.status(), StatusCode::OK);
    let csrf_cookie = first
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("tor-dev-csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html =
        String::from_utf8(to_bytes(first.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
    // Internal RIL consent remains independent of the visible account UI.
    assert!(!html.contains("Анализ ваших переходов"));
    assert!(!html.contains("Приватность"));
    let csrf = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let cookies = format!("{session}; {csrf_cookie}");
    let enable = format!("choice=enable&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, "", ORIGIN)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // A valid CSRF token without an authenticated session is not consent.
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, &csrf_cookie, ORIGIN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/me/behavior",
            &enable,
            &cookies,
            "https://invalid.example"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (true, 1)
    );
    for _ in 0..3 {
        assert_eq!(
            send(&app, "GET", "/me", "", &cookies, ORIGIN)
                .await
                .status(),
            StatusCode::OK
        );
    }
    let report = tor_web::ril_path_report_for_diagnostics().unwrap();
    assert_eq!(report.assessed_events, 3);
    assert_eq!(report.active_subjects, 1);

    let disable = format!("choice=disable&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", "/me/behavior", &disable, &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 2)
    );
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .active_subjects,
        0
    );
    assert_eq!(
        send(&app, "GET", "/me", "", &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .assessed_events,
        3
    );

    accounts.revoke_session(&digest).await.unwrap();
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
