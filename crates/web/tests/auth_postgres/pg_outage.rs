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
    let witness = sqlx::PgPool::connect(&url).await.unwrap();
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM users WHERE email_normalized=$1",
        )
        .bind(&email)
        .fetch_one(&witness)
        .await
        .unwrap(),
        0,
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
    assert!(
        created
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap_or("").starts_with("tor-dev-session=")),
    );
    let (user_id, session_count): (i64, i64) = sqlx::query_as(
        "SELECT u.id, (SELECT count(*) FROM sessions s WHERE s.user_id=u.id)
         FROM users u WHERE u.email_normalized=$1",
    )
    .bind(&email)
    .fetch_one(&witness)
    .await
    .unwrap();
    assert_eq!(session_count, 1);

    let repeated = send(
        &healthy_app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_ne!(repeated.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sessions WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&witness)
        .await
        .unwrap(),
        1,
        "a rejected duplicate must not mint a second session"
    );
    witness.close().await;
}
