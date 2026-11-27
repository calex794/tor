//! MAIL-01h2: exercise real web router, operator ACL, CSRF and PostgreSQL audit.
use super::*;

const PAGE: &str = "/odmen/system-mail";

async fn http(
    app: &Router,
    method: &str,
    path: &str,
    session: Option<&str>,
    csrf: &str,
    origin: &str,
    body: String,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(token) = session {
        builder = builder.header(
            header::COOKIE,
            format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        );
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let html = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (status, String::from_utf8(html.to_vec()).unwrap())
}

fn form(csrf: &str, revision: i64, limit: i64) -> String {
    format!(
        "csrf_token={csrf}&revision={revision}&warning_1=100&warning_2=300&news_pause=400&monthly_limit={limit}"
    )
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn mail_01_h2_admin_http_limits_stop_and_audit() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool.clone()),
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('mail-budget-{nonce}@example.invalid','Mail budget admin','$argon2id$ci')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let session = generate_session_token();
    let csrf = generate_session_token();
    let digest = hex::encode(session_token_digest(&session));
    pg(
        &url,
        &format!(
            "INSERT INTO sessions(token_hash,user_id,expires_at)
             VALUES(decode('{digest}','hex'),{actor},clock_timestamp()+interval '1 day')"
        ),
    );
    let token = Some(session.as_str());
    assert_eq!(
        http(&app, "GET", PAGE, None, &csrf, ORIGIN, String::new())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http(&app, "GET", PAGE, token, &csrf, ORIGIN, String::new())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({actor},'administrator',true)"
        ),
    );
    // Set an isolated test budget and simulated month, not a real sending profile.
    pg(
        &url,
        "UPDATE mail_cost_policy SET enabled=false,warning_1=10000,warning_2=30000,
         news_pause=40000,monthly_limit=50000 WHERE singleton;
         INSERT INTO mail_cost_months
           (month_start,free_allowance,baseline_kopecks,rate_per_thousand,reserved_attempts)
         VALUES(date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date,
                2000,0,8032,5000)
         ON CONFLICT(month_start) DO UPDATE SET free_allowance=2000,baseline_kopecks=0,
                rate_per_thousand=8032,reserved_attempts=5000",
    );
    let revision: i64 = pg(
        &url,
        "SELECT revision FROM mail_cost_policy WHERE singleton",
    )
    .parse()
    .unwrap();
    let (status, html) = http(&app, "GET", PAGE, token, &csrf, ORIGIN, String::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("240.96 ₽"));
    assert!(html.contains("259.04 ₽"));
    assert!(html.contains("Первый порог расходов"));
    assert!(html.contains("Отправка остановлена"));
    assert!(!html.contains("name=\"enabled\""));

    for (posted, origin) in [
        (form("bad", revision, 600), ORIGIN),
        (form(&csrf, revision, 600), "https://evil.invalid"),
    ] {
        assert_eq!(
            http(&app, "POST", PAGE, token, &csrf, origin, posted)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        http(
            &app,
            "POST",
            PAGE,
            token,
            &csrf,
            ORIGIN,
            form(&csrf, revision, 300)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http(
            &app,
            "POST",
            PAGE,
            token,
            &csrf,
            ORIGIN,
            format!("{}&enabled=true", form(&csrf, revision, 600)),
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Concurrent changes with one expected revision cannot overwrite each other.
    let (a, b) = tokio::join!(
        http(
            &app,
            "POST",
            PAGE,
            token,
            &csrf,
            ORIGIN,
            form(&csrf, revision, 600)
        ),
        http(
            &app,
            "POST",
            PAGE,
            token,
            &csrf,
            ORIGIN,
            form(&csrf, revision, 700)
        ),
    );
    assert!(matches!(
        (a.0, b.0),
        (StatusCode::SEE_OTHER, StatusCode::CONFLICT)
            | (StatusCode::CONFLICT, StatusCode::SEE_OTHER)
    ));
    assert_eq!(
        pg(&url, "SELECT enabled FROM mail_cost_policy WHERE singleton"),
        "f",
        "saving limits must never enable sending"
    );
    // A stop request does not depend on a stale browser revision.
    pg(
        &url,
        "UPDATE mail_cost_policy SET enabled=true WHERE singleton",
    );
    let stop = format!("{PAGE}/stop");
    for _ in 0..2 {
        assert_eq!(
            http(
                &app,
                "POST",
                &stop,
                token,
                &csrf,
                ORIGIN,
                format!("csrf_token={csrf}")
            )
            .await
            .0,
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        pg(&url, "SELECT enabled FROM mail_cost_policy WHERE singleton"),
        "f"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM mail_cost_admin_audit WHERE actor_user_id={actor}")
        ),
        "2"
    );
    pg_audit_mutation_rejected(
        &url,
        &format!(
            "UPDATE mail_cost_admin_audit SET new_monthly_limit=1 WHERE actor_user_id={actor}"
        ),
    );
    pg_audit_mutation_rejected(
        &url,
        &format!("DELETE FROM mail_cost_admin_audit WHERE actor_user_id={actor}"),
    );
    let reconcile_path = format!("{PAGE}/reconcile");
    let rev: i64 = pg(
        &url,
        "SELECT revision FROM mail_cost_policy WHERE singleton",
    )
    .parse()
    .unwrap();
    let month = pg(
        &url,
        "SELECT to_char(clock_timestamp() AT TIME ZONE 'Europe/Moscow','YYYY-MM')",
    );
    let evidence = "ab".repeat(32);
    let reconciliation = |csrf: &str, attempts: i64, rev: i64| {
        format!(
            "csrf_token={csrf}&revision={rev}&month={month}&free_allowance=1000&baseline_kopecks=100&rate_per_thousand=8032&reserved_attempts={attempts}&evidence_sha256={evidence}&reviewed=yes"
        )
    };
    for (posted, origin) in [
        (reconciliation("bad", 5001, rev), ORIGIN),
        (reconciliation(&csrf, 5001, rev), "https://evil.invalid"),
    ] {
        assert_eq!(
            http(&app, "POST", &reconcile_path, token, &csrf, origin, posted)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for (posted, expected) in [
        (reconciliation(&csrf, 4999, rev), StatusCode::BAD_REQUEST),
        (reconciliation(&csrf, 5001, rev - 1), StatusCode::CONFLICT),
        (
            reconciliation(&csrf, 5001, rev).replace("reviewed=yes", "reviewed=no"),
            StatusCode::BAD_REQUEST,
        ),
        (reconciliation(&csrf, 5001, rev), StatusCode::SEE_OTHER),
        (reconciliation(&csrf, 5001, rev), StatusCode::CONFLICT),
    ] {
        assert_eq!(
            http(&app, "POST", &reconcile_path, token, &csrf, ORIGIN, posted)
                .await
                .0,
            expected
        );
    }
    assert_eq!(
        pg(&url, "SELECT enabled FROM mail_cost_policy WHERE singleton"),
        "f"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM mail_cost_month_reconciliation_audit WHERE actor_user_id={actor}")
        ),
        "1"
    );
    pg(
        &url,
        &format!("UPDATE account_roles SET is_active=false WHERE user_id={actor}"),
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &reconcile_path,
            token,
            &csrf,
            ORIGIN,
            reconciliation(&csrf, 5002, rev + 1),
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &stop,
            token,
            &csrf,
            ORIGIN,
            format!("csrf_token={csrf}")
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Leave subsequent suites a disabled, empty month with default limits.
    pg(
        &url,
        "UPDATE mail_cost_policy SET enabled=false,warning_1=10000,warning_2=30000,
        news_pause=40000,monthly_limit=50000 WHERE singleton; DELETE FROM mail_cost_months",
    );
    pg(
        &url,
        &format!("DELETE FROM account_roles WHERE user_id={actor}"),
    );
    pg(&url, &format!("DELETE FROM users WHERE id={actor}"));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci and CI-only master key"]
async fn mail_03_profile_http_save_never_sends_or_reveals_credentials() {
    use tor_accounts::services::security::hash_password;
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(
        env::var("TOR_POSTBOX_PROFILE_MASTER_KEY").unwrap(),
        "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
    );
    assert_eq!(env::var("TOR_POSTBOX_PROFILE_KEY_VERSION").unwrap(), "1");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool),
        false,
        ORIGIN.to_owned(),
    );
    pg(
        &url,
        "UPDATE mail_cost_policy SET enabled=false WHERE singleton",
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = "FixturePasswordMail03";
    let hash = hash_password(password).unwrap();
    let actor: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('mail-profile-http-{nonce}@example.invalid','Postbox HTTP fixture','{hash}')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let session = generate_session_token();
    let csrf = generate_session_token();
    let digest = hex::encode(session_token_digest(&session));
    pg(
        &url,
        &format!(
            "INSERT INTO sessions(token_hash,user_id,expires_at)
             VALUES(decode('{digest}','hex'),{actor},clock_timestamp()+interval '1 day')"
        ),
    );
    let token = Some(session.as_str());
    let revision: i64 = pg(
        &url,
        "SELECT revision FROM mail_postbox_profile WHERE singleton",
    )
    .parse()
    .unwrap();
    let posted = |csrf: &str, password: &str| {
        format!(
            "csrf_token={csrf}&revision={revision}&identity_id=ci-http&sender=sender%40example.invalid&access=ci-access-fixture&secret=ci-secret-fixture&current_password={password}"
        )
    };
    let path = "/odmen/system-mail/profile";
    assert_eq!(
        http(
            &app,
            "POST",
            path,
            token,
            &csrf,
            ORIGIN,
            posted(&csrf, password)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({actor},'administrator',true)"
        ),
    );
    for (body, origin, expected) in [
        (posted("bad", password), ORIGIN, StatusCode::FORBIDDEN),
        (
            posted(&csrf, password),
            "https://evil.invalid",
            StatusCode::FORBIDDEN,
        ),
        (
            posted(&csrf, "wrong-password"),
            ORIGIN,
            StatusCode::UNAUTHORIZED,
        ),
        (posted(&csrf, password), ORIGIN, StatusCode::SEE_OTHER),
        (posted(&csrf, password), ORIGIN, StatusCode::CONFLICT),
    ] {
        let (status, html) = http(&app, "POST", path, token, &csrf, origin, body).await;
        assert_eq!(status, expected);
        assert!(!html.contains("ci-secret-fixture"));
        assert!(!html.contains(password));
    }
    let (status, html) = http(&app, "GET", PAGE, token, &csrf, ORIGIN, String::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("sender@example.invalid"));
    assert!(html.contains("/odmen/system-mail/probe"));
    assert!(!html.contains("ci-access-fixture"));
    assert!(!html.contains("ci-secret-fixture"));
    assert!(!html.contains("name=\"enabled\""));
    // Stale revision must fail before any provider I/O.
    let probe = format!("csrf_token={csrf}&revision={revision}");
    assert_eq!(
        http(
            &app,
            "POST",
            "/odmen/system-mail/probe",
            token,
            &csrf,
            ORIGIN,
            probe,
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(&url, "SELECT enabled FROM mail_cost_policy WHERE singleton"),
        "f"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM mail_postbox_profile_audit WHERE actor_user_id={actor}"),
        ),
        "1"
    );
    pg(
        &url,
        "UPDATE mail_postbox_profile SET identity_id=NULL,sender=NULL,key_version=NULL,
         nonce=NULL,ciphertext=NULL,saved_at=NULL WHERE singleton",
    );
    pg(
        &url,
        &format!("DELETE FROM account_roles WHERE user_id={actor}"),
    );
    pg(&url, &format!("DELETE FROM users WHERE id={actor}"));
}
