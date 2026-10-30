//! Dedicated real-PostgreSQL account validation checks. Never tests production data.
use super::*;

pub(super) async fn invalid_registration(app: &Router, csrf: &str, csrf_cookie: &str) {
    let weak_password = format!(
        "email=tor_ci_account%40example.com&display_name=CIReader&password=1234&csrf_token={csrf}"
    );
    let invalid = send(
        app,
        request("POST", "/register", &weak_password, csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(invalid.headers()[header::CACHE_CONTROL], "no-store");
    let invalid_html = body_of(invalid).await;
    assert!(invalid_html.contains("<html lang=\"ru\">"));
    assert!(invalid_html.contains("role=\"alert\""));
    assert!(invalid_html.contains("Пароль должен содержать от 12 до 128 символов"));
    assert!(invalid_html.contains("value=\"tor_ci_account@example.com\""));
    assert!(!invalid_html.contains("value=\"1234\""));
    // Untrusted input is escaped even when the server rejects a weak password.
    let unsafe_name =
        format!("email=x%40example.com&display_name=%3Cscript%3E&password=1234&csrf_token={csrf}");
    let bad_name = send(
        app,
        request("POST", "/register", &unsafe_name, csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(bad_name.status(), StatusCode::BAD_REQUEST);
    let escaped_html = body_of(bad_name).await;
    assert!(escaped_html.contains("&lt;script&gt;"));
    assert!(!escaped_html.contains("value=\"<script>\""));
}

/// F-09: an invalid manual code must keep the pending guest invitation and
/// must not create an account before the visitor corrects the form.
async fn invalid_manual_invitation(app: &Router, csrf: &str, cookies: &str, email: &str) {
    for (code, message) in [
        ("broken+code", "Некорректный код приглашения"),
        ("unknown_invite_2026", "Код приглашения не найден"),
    ] {
        let form = format!(
            "email={}&display_name=CIReader&password=LongSecurePassword2026&manual_invite_code={code}&csrf_token={csrf}",
            email.replace('@', "%40")
        );
        let response = send(app, request("POST", "/register", &form, cookies, ORIGIN)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers().get_all(header::SET_COOKIE).iter().next().is_none(),
            "rejected signup must not reset the valid guest invitation"
        );
        let html = body_of(response).await;
        assert!(html.contains(message));
        assert!(html.contains(format!("value=\"{email}\"").as_str()));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(!html.contains("value=\"LongSecurePassword2026\""));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_http_invite_validation_and_consumed_guest_cookie() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "do not modify a production database");
    let catalog = PgCatalog::connect(&url).await.expect("migrated PostgreSQL");
    let reader = PgReader::new(catalog.pool().clone());
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let html = body_of(page).await;
    let csrf = csrf_token(&html);
    let guest = tor_accounts::services::security::generate_session_token();
    let cookies = format!("{csrf_cookie}; tor-dev-guest={guest}");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let email = format!("f09-http-{stamp}@example.invalid");

    invalid_manual_invitation(&app, csrf, &cookies, &email).await;

    // Retrying without the invalid manual code must succeed with the same
    // guest identity; this also proves the failed POST did not create a user.
    let form = format!(
        "email={}&display_name=CIReader&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40"),
    );
    let response = send(&app, request("POST", "/register", &form, &cookies, ORIGIN)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/me");
    assert!(cookie(&response, "tor-dev-session=").len() > 20);
    assert_eq!(cookie(&response, "tor-dev-guest="), "tor-dev-guest=");
    let cleared = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find(|value| value.to_str().unwrap_or("").starts_with("tor-dev-guest="))
        .expect("explicit guest cookie expiration")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cleared.contains("Max-Age=0"));
    assert!(cleared.contains("SameSite=Lax"));
    assert!(cleared.contains("HttpOnly"));
}

pub(super) async fn duplicate_registration(app: &Router, form: &str, csrf_cookie: &str) {
    let duplicate = send(app, request("POST", "/register", form, csrf_cookie, ORIGIN)).await;
    assert_eq!(
        duplicate.status(),
        StatusCode::CONFLICT,
        "Email uniqueness enforced by PostgreSQL"
    );
    assert_eq!(duplicate.headers()[header::CACHE_CONTROL], "no-store");
    let error_html = body_of(duplicate).await;
    assert!(error_html.contains("<html lang=\"ru\">"));
    assert!(error_html.contains("account-error__card"));
    assert!(error_html.contains("Не удалось завершить регистрацию"));
    assert!(error_html.contains("href=\"/register\""));
    assert!(error_html.contains("href=\"/login\""));
    assert!(!error_html.contains("tor_ci_account@example.com"));
}

pub(super) async fn rejected_login(app: &Router, bad_login: &str, login_csrf_cookie: &str) {
    let bad = send(
        app,
        request("POST", "/login", bad_login, login_csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(bad.headers()[header::CACHE_CONTROL], "no-store");
    let login_error = body_of(bad).await;
    assert!(login_error.contains("account-error__card"));
    assert!(login_error.contains("Неверный адрес почты или пароль"));
    assert!(login_error.contains("href=\"/login\""));
}

pub(super) async fn seed_exhausted_proxy_counter(accounts: &PgAccounts) {
    // Nginx connects from 127.0.0.1 for all real website visitors.
    // A previously exhausted shared IP counter must not lock a real user out.
    // This fixture touches only the disposable tor_ci database.
    let key = tor_accounts::services::security::session_token_digest("auth:login:peer:127.0.0.1");
    for _ in 0..61 {
        accounts
            .permit_auth_attempt(&key, 60, 600)
            .await
            .expect("seed previously shared Nginx peer limit");
    }
}
