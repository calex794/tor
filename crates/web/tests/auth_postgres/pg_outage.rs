//! S002/R1: an unavailable PostgreSQL connection cannot mint a login session.
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn app_with_pool(catalog: PgCatalog) -> Router {
    let pool = catalog.pool();
    tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool),
        false,
        ORIGIN.to_owned(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated TEST_DATABASE_URL PostgreSQL tor_ci"]
async fn a_02_registration_pg_pool_outage_never_mints_session_then_recovers() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let closing_pool = catalog.pool();
    let offline_app = app_with_pool(catalog);
    let witness_catalog = PgCatalog::connect(&url).await.unwrap();
    let witness = PgAccounts::from_pool(witness_catalog.pool());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("a02-pg-outage-{nonce}@example.invalid");
    let form_email = email.replace('@', "%40");
    let page = send(&offline_app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let html = body_of(page).await;
    let token = csrf_token(&html).to_owned();
    let form = format!(
        "email={form_email}&display_name=OutageReader&password=LongSecurePassword2026&csrf_token={token}"
    );

    // Only this test's pool is disconnected; the shared disposable PostgreSQL
    // service stays available to the independent database witness.
    closing_pool.close().await;
    let rejected = send(
        &offline_app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        rejected
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .all(|value| !value.to_str().unwrap_or("").starts_with("tor-dev-session=")),
        "failed registration must never return a session cookie"
    );
    assert!(
        witness.find_user(&email).await.unwrap().is_none(),
        "failed registration must not persist an account"
    );

    // Rebuild the router with a fresh live pool: this is equivalent to a
    // recovered database connection, not a production restart.
    let healthy_app = app_with_pool(PgCatalog::connect(&url).await.unwrap());
    let created = send(
        &healthy_app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let session = cookie(&created, "tor-dev-session=");
    let digest = tor_accounts::services::security::session_token_digest(
        session.split_once('=').expect("session cookie pair").1,
    );
    let user_id = witness
        .find_user(&email)
        .await
        .unwrap()
        .expect("registered user")
        .id;
    assert_eq!(
        witness.active_session_user(&digest).await.unwrap(),
        Some(user_id)
    );
    let sessions = witness
        .list_active_sessions(user_id, &digest)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].current);

    let repeated = send(
        &healthy_app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_ne!(repeated.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        witness
            .list_active_sessions(user_id, &digest)
            .await
            .unwrap()
            .len(),
        1,
        "a rejected duplicate must not mint a second session"
    );
    witness_catalog.pool().close().await;
}
