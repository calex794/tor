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
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .next()
                .is_none(),
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
    assert!(
        url.ends_with("/tor_ci"),
        "do not modify a production database"
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_http_personal_referral_link_and_private_aggregate() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never test production");
    let catalog = PgCatalog::connect(&url).await.expect("migrated PostgreSQL");
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let referrals = tor_referrals::repositories::PgReferrals::new(pool.clone());
    let reader = PgReader::new(pool);
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );

    let anonymous = send(
        &app,
        request("GET", "/me/referrals", "", "", ORIGIN),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);
    assert_eq!(anonymous.headers()[header::LOCATION], "/login");
    assert_eq!(
        send(
            &app,
            request("GET", "/api/v1/me/referrals", "", "", ORIGIN),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let owner_email = format!("f09-owner-{stamp}@example.invalid");
    let register_page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    let register_cookie = cookie(&register_page, "tor-dev-csrf=");
    let register_body = body_of(register_page).await;
    let register_csrf = csrf_token(&register_body);
    let owner_form = format!(
        "email={}&display_name=Owner&password=LongSecurePassword2026&csrf_token={register_csrf}",
        owner_email.replace('@', "%40"),
    );
    let registered = send(
        &app,
        request("POST", "/register", &owner_form, &register_cookie, ORIGIN),
    )
    .await;
    assert_eq!(registered.status(), StatusCode::SEE_OTHER);
    let owner_session = cookie(&registered, "tor-dev-session=");
    let owner_id = accounts
        .find_user(&owner_email)
        .await
        .unwrap()
        .expect("new owner")
        .id;

    let initial = send(
        &app,
        request("GET", "/me/referrals", "", &owner_session, ORIGIN),
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(initial.headers()[header::CACHE_CONTROL], "no-store");
    let own_csrf_cookie = cookie(&initial, "tor-dev-csrf=");
    let initial_html = body_of(initial).await;
    assert!(initial_html.contains("method=\"post\" action=\"/me/referrals\""));
    assert!(initial_html.contains("Создать ссылку"));
    assert!(initial_html.contains("href=\"/me/referrals\""));
    let token = csrf_token(&initial_html).to_owned();
    let own_cookies = format!("{owner_session}; {own_csrf_cookie}");
    let post_form = format!("csrf_token={token}");
    assert_eq!(
        send(
            &app,
            request("POST", "/me/referrals", &post_form, &own_cookies, "https://evil.example"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            request("POST", "/api/v1/me/referrals", "csrf_token=invalid", &own_cookies, ORIGIN),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(referrals.default_reader_code(owner_id).await.unwrap().is_none());

    let created = send(
        &app,
        request("POST", "/me/referrals", &post_form, &own_cookies, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert_eq!(created.headers()[header::LOCATION], "/me/referrals");
    let owned = referrals
        .reader_overview(owner_id)
        .await
        .unwrap()
        .default_code
        .expect("owned invitation");
    assert_eq!(owned.status, "active");
    assert_eq!(
        send(
            &app,
            request("POST", "/me/referrals", &post_form, &own_cookies, ORIGIN),
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let api_create = send(
        &app,
        request("POST", "/api/v1/me/referrals", &post_form, &own_cookies, ORIGIN),
    )
    .await;
    assert_eq!(api_create.status(), StatusCode::OK);
    assert_eq!(api_create.headers()[header::CACHE_CONTROL], "no-store");
    let data = body_of(api_create).await;
    assert!(data.contains(&format!("\"code\":\"{}\"", owned.code)));
    assert!(data.contains("\"active\":0"));
    assert_eq!(
        referrals.default_reader_code(owner_id).await.unwrap(),
        Some(owned.clone()),
    );

    let landing = send(
        &app,
        request("GET", &format!("/r/{}", owned.code), "", "", ORIGIN),
    )
    .await;
    assert_eq!(landing.status(), StatusCode::SEE_OTHER);
    let guest_cookie = cookie(&landing, "tor-dev-guest=");
    let guest_page = send(
        &app,
        request("GET", "/register", "", &guest_cookie, ORIGIN),
    )
    .await;
    let guest_csrf_cookie = cookie(&guest_page, "tor-dev-csrf=");
    let guest_html = body_of(guest_page).await;
    let guest_csrf = csrf_token(&guest_html);
    let invited_email = format!("f09-invited-{stamp}@example.invalid");
    let invited_form = format!(
        "email={}&display_name=Invited&password=LongSecurePassword2026&csrf_token={guest_csrf}",
        invited_email.replace('@', "%40"),
    );
    let invited = send(
        &app,
        request(
            "POST",
            "/register",
            &invited_form,
            &format!("{guest_cookie}; {guest_csrf_cookie}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(invited.status(), StatusCode::SEE_OTHER);
    let invited_session = cookie(&invited, "tor-dev-session=");
    let invited_id = accounts
        .find_user(&invited_email)
        .await
        .unwrap()
        .expect("invited account")
        .id;
    // The recovery worker can beat this assertion. Replay is idempotent.
    let _ = referrals.finalize_one(invited_id).await.unwrap();

    let overview = referrals.reader_overview(owner_id).await.unwrap();
    assert_eq!((overview.active, overview.blocked), (1, 0));
    let own_page = send(
        &app,
        request("GET", "/me/referrals", "", &owner_session, ORIGIN),
    )
    .await;
    let html = body_of(own_page).await;
    assert!(html.contains("Активных привязок: <strong>1</strong>"));
    assert!(html.contains(&format!("{ORIGIN}/r/{}", owned.code)));
    assert!(!html.contains(&invited_email));
    let other_api = send(
        &app,
        request("GET", "/api/v1/me/referrals", "", &invited_session, ORIGIN),
    )
    .await;
    assert_eq!(other_api.status(), StatusCode::OK);
    let other = body_of(other_api).await;
    assert!(other.contains("\"invitation\":null"));
    assert!(other.contains("\"active\":0"));
    assert!(!other.contains(&owned.code));
    assert!(!other.contains(&owner_email));
}
