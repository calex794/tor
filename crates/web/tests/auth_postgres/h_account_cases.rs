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
        .unwrap();
    assert!(cleared.contains("Max-Age=0"));
    assert!(cleared.contains("SameSite=Lax"));
    assert!(cleared.contains("HttpOnly"));
}

/// End-to-end Axum + real PostgreSQL: an external second referral click
/// cannot replace the initial guest capture before an email registration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_http_first_touch_is_attributed_on_signup() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "do not modify a production database");
    let catalog = PgCatalog::connect(&url).await.expect("migrated PostgreSQL");
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let reader = PgReader::new(pool.clone());
    let referrals = tor_referrals::repositories::PgReferrals::new(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let mut inviters = Vec::new();
    for role in ["first", "second"] {
        let inviter: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash) \
             VALUES($1,'F09 inviter','fixture-only') RETURNING id",
        )
        .bind(format!("f09-{role}-{stamp}@example.invalid"))
        .fetch_one(&pool)
        .await
        .expect("seed inviter");
        let code = format!("f09h_{inviter}");
        sqlx::query(
            "INSERT INTO referral_codes(code,owner_user_id,program_type) \
             VALUES($1,$2,'reader_spend')",
        )
        .bind(&code)
        .bind(inviter)
        .execute(&pool)
        .await
        .expect("seed active code");
        inviters.push((inviter, code));
    }

    let first = send(
        &app,
        request("GET", &format!("/r/{}", inviters[0].1), "", "", ORIGIN),
    )
    .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    assert_eq!(first.headers()[header::LOCATION], "/register");
    let guest = cookie(&first, "tor-dev-guest=");
    let set_guest = first
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find(|v| v.to_str().unwrap_or("").starts_with("tor-dev-guest="))
        .expect("guest cookie")
        .to_str()
        .unwrap();
    assert!(set_guest.contains("SameSite=Lax"));
    assert!(set_guest.contains("HttpOnly"));

    let second = send(
        &app,
        request("GET", &format!("/r/{}", inviters[1].1), "", &guest, ORIGIN),
    )
    .await;
    assert_eq!(second.status(), StatusCode::SEE_OTHER);
    assert!(second.headers().get_all(header::SET_COOKIE).iter().next().is_none());
    let token = guest.strip_prefix("tor-dev-guest=").expect("cookie name");
    let digest = tor_accounts::services::security::session_token_digest(token);
    let selected: String = sqlx::query_scalar(
        "SELECT referral_code FROM referral_captures \
         WHERE guest_hash=$1 AND status='active'",
    )
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .expect("one active first-touch capture");
    assert_eq!(selected, inviters[0].1);

    let page = send(&app, request("GET", "/register", "", &guest, ORIGIN)).await;
    let csrf = csrf_token(&body_of(page.clone()).await).to_owned();
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let email = format!("f09-new-{stamp}@example.invalid");
    let signup = format!(
        "email={}&display_name=Invited&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40"),
    );
    let reply = send(
        &app,
        request(
            "POST",
            "/register",
            &signup,
            &format!("{csrf_cookie}; {guest}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::SEE_OTHER);
    assert_eq!(cookie(&reply, "tor-dev-guest="), "tor-dev-guest=");

    let subject: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email_normalized=$1")
        .bind(&email)
        .fetch_one(&pool)
        .await
        .expect("registered subject");
    // The background worker may have processed the event already; both
    // executions must converge on a single stable attribution.
    let _ = referrals.finalize_one(subject).await.expect("consume signup outbox");
    let inviter: i64 = sqlx::query_scalar(
        "SELECT referrer_user_id FROM referral_attributions \
         WHERE program_type='reader_spend' AND subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .expect("durable referral attribution");
    assert_eq!(inviter, inviters[0].0);
    assert!(!referrals.finalize_one(subject).await.unwrap());
    let result: String = sqlx::query_scalar(
        "SELECT result FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "attributed");
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
