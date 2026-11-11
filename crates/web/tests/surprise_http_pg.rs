//! F-SURPRISE: actual A/F HTTP + PostgreSQL acceptance with the opt-in gate ON.
//! Only run against isolated tor_ci; no deployment or production actors.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use base64::Engine;
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, statement: &str) -> String {
    let out = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            statement,
        ])
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "disposable fixture SQL failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

async fn http(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    auth: (&str, &str),
    body: &str,
    json_body: bool,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, auth.0)
                .header("x-csrf-token", auth.1)
                .header(
                    header::CONTENT_TYPE,
                    if json_body {
                        "application/json"
                    } else {
                        "application/x-www-form-urlencoded"
                    },
                )
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn private(response: &Response, code: StatusCode) {
    assert_eq!(response.status(), code);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    // Private A/F responses have an explicit no-store and unframeable CSP;
    // an x-robots-tag is not part of the shared private_response contract.
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
}

async fn parse(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 512 * 1024).await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated disposable PostgreSQL 18 tor_ci"]
async fn surprise_http_opt_in_admin_controls_and_reader_one_shot_are_live() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated tor_ci only");
    assert!(
        url.starts_with("postgresql://")
            && url.contains("@127.0.0.1:5432/")
            && url.ends_with("/tor_ci"),
        "refuse non-isolated database"
    );
    std::env::set_var(
        "TOR_SURPRISE_HMAC_KEY",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0x43_u8; 32]),
    );
    std::env::set_var("TOR_SURPRISE_HTTP_ENABLED", "0");

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let csrf = generate_session_token();
    let user = accounts
        .register_with_session(
            &format!("surprise-http-{nonce}@tor-ci.example"),
            "Surprise HTTP acceptance",
            &hash_password("SurpriseHttpAcceptancePass2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );
    // The runtime opt-in is independent of the schema kill switch.
    let response = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &cookie,
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&response, StatusCode::NOT_FOUND);
    std::env::set_var("TOR_SURPRISE_HTTP_ENABLED", "1");

    pg(&url, "UPDATE surprise_runtime_policy SET enabled=false,probability_per_mille=350 WHERE singleton");
    let response = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &cookie,
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&response, StatusCode::OK);
    assert_eq!(parse(response).await["state"], "off");

    let stranger = http(
        &app,
        "GET",
        "/admin/surprises",
        &cookie,
        (ORIGIN, &csrf),
        "",
        false,
    )
    .await;
    private(&stranger, StatusCode::FORBIDDEN);
    let guest = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        "",
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&guest, StatusCode::UNAUTHORIZED);

    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({user},'administrator',true)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=true"
        ),
    );

    let title = format!("surprise-http-{nonce}");
    let create = format!(
        "csrf_token={csrf}&kind=joke&title={title}&body=Safe+editorial+joke&book_id=&editorial=on"
    );
    let wrong = http(
        &app,
        "POST",
        "/admin/surprises/create",
        &cookie,
        ("https://other.example", &csrf),
        &create,
        false,
    )
    .await;
    private(&wrong, StatusCode::FORBIDDEN);
    let created = http(
        &app,
        "POST",
        "/admin/surprises/create",
        &cookie,
        (ORIGIN, &csrf),
        &create,
        false,
    )
    .await;
    private(&created, StatusCode::SEE_OTHER);
    let item_id: i64 = pg(
        &url,
        &format!("SELECT id FROM surprise_content WHERE title='{title}' AND state='draft'"),
    )
    .parse()
    .unwrap();

    let approval = format!("csrf_token={csrf}&state=approved");
    let approved = http(
        &app,
        "POST",
        &format!("/admin/surprises/{item_id}/state"),
        &cookie,
        (ORIGIN, &csrf),
        &approval,
        false,
    )
    .await;
    private(&approved, StatusCode::SEE_OTHER);

    let control = format!("csrf_token={csrf}&enabled=on&probability_per_mille=1000");
    let enabled = http(
        &app,
        "POST",
        "/admin/surprises/policy",
        &cookie,
        (ORIGIN, &csrf),
        &control,
        false,
    )
    .await;
    private(&enabled, StatusCode::SEE_OTHER);

    let response = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &cookie,
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&response, StatusCode::OK);
    let plan = parse(response).await;
    assert_eq!(plan["state"], "planned");
    assert_eq!(plan["csrf_token"], csrf);
    let day = plan["utc_day"].as_i64().unwrap();
    let seconds = plan["active_after_seconds"].as_i64().unwrap();
    assert!((120..=1200).contains(&seconds));

    let attempted = json!({"utc_day":day,"active_seconds":1200}).to_string();
    let bad_csrf = http(
        &app,
        "POST",
        "/api/v1/me/surprises/claim",
        &cookie,
        (ORIGIN, "not-a-valid-token"),
        &attempted,
        true,
    )
    .await;
    private(&bad_csrf, StatusCode::FORBIDDEN);
    let early = http(
        &app,
        "POST",
        "/api/v1/me/surprises/claim",
        &cookie,
        (ORIGIN, &csrf),
        &attempted,
        true,
    )
    .await;
    private(&early, StatusCode::OK);
    assert_eq!(parse(early).await["accepted"], false);

    // Only the disposable test row is backdated; the public site never
    // offers time travel or a second independent chance.
    pg(
        &url,
        &format!(
            "UPDATE surprise_daily_decisions
         SET created_at=now()-interval '30 minutes'
         WHERE user_id={user} AND utc_day={day}"
        ),
    );
    let claimed = http(
        &app,
        "POST",
        "/api/v1/me/surprises/claim",
        &cookie,
        (ORIGIN, &csrf),
        &attempted,
        true,
    )
    .await;
    private(&claimed, StatusCode::OK);
    assert_eq!(parse(claimed).await["accepted"], true);
    let repeated = http(
        &app,
        "POST",
        "/api/v1/me/surprises/claim",
        &cookie,
        (ORIGIN, &csrf),
        &attempted,
        true,
    )
    .await;
    private(&repeated, StatusCode::OK);
    assert_eq!(parse(repeated).await["accepted"], false);

    let open = json!({"utc_day":day}).to_string();
    let response = http(
        &app,
        "POST",
        "/api/v1/me/surprises/open",
        &cookie,
        (ORIGIN, &csrf),
        &open,
        true,
    )
    .await;
    private(&response, StatusCode::OK);
    let gift = parse(response).await;
    assert_eq!(gift["id"], item_id);
    assert_eq!(gift["title"], title);
    assert_eq!(gift["editorial"], true);
    assert!(gift["book_id"].is_null());

    // Switching an active legacy-day decision to randomized experiment
    // must never re-roll it into another treatment slot or leak an opened
    // legacy gift through the experiment. The control arm has no prize.
    let pilot_config =
        format!("csrf_token={csrf}&enabled=on&probability_per_mille=1000&experiment_enabled=on");
    let activated = http(
        &app,
        "POST",
        "/admin/surprises/policy",
        &cookie,
        (ORIGIN, &csrf),
        &pilot_config,
        false,
    )
    .await;
    private(&activated, StatusCode::SEE_OTHER);

    let old_plan = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &cookie,
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&old_plan, StatusCode::OK);
    assert_eq!(parse(old_plan).await["state"], "none");

    let old_open = http(
        &app,
        "POST",
        "/api/v1/me/surprises/open",
        &cookie,
        (ORIGIN, &csrf),
        &open,
        true,
    )
    .await;
    private(&old_open, StatusCode::NOT_FOUND);

    let pilot_token = generate_session_token();
    let pilot_csrf = generate_session_token();
    let pilot_user = PgAccounts::from_pool(pool.clone())
        .register_with_session(
            &format!("surprise-pilot-{nonce}@tor-ci.example"),
            "Surprise randomized pilot",
            &hash_password("SurprisePilotAcceptancePass2026").unwrap(),
            &session_token_digest(&pilot_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let pilot_cookie = format!("tor-dev-session={pilot_token}; tor-dev-csrf={pilot_csrf}");
    let assigned = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &pilot_cookie,
        (ORIGIN, &pilot_csrf),
        "",
        true,
    )
    .await;
    private(&assigned, StatusCode::OK);
    let response = parse(assigned).await;
    let arm = pg(
        &url,
        &format!("SELECT experiment_arm FROM surprise_daily_decisions WHERE user_id={pilot_user}"),
    );
    assert!(
        ["control", "gentle", "standard"].contains(&arm.as_str()),
        "{arm}"
    );
    assert_eq!(
        response["state"],
        if arm == "control" { "none" } else { "planned" }
    );
    let repeat = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &pilot_cookie,
        (ORIGIN, &pilot_csrf),
        "",
        true,
    )
    .await;
    private(&repeat, StatusCode::OK);
    assert_eq!(parse(repeat).await["state"], response["state"]);
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM surprise_daily_decisions WHERE user_id={pilot_user}")
        ),
        "1",
        "navigation must not create an extra draw",
    );
    pg(&url, &format!("DELETE FROM users WHERE id={pilot_user}"));
    pg(
        &url,
        "UPDATE surprise_runtime_policy SET experiment_enabled=false WHERE singleton",
    );

    let disabled = http(
        &app,
        "POST",
        "/api/v1/me/surprises/disable",
        &cookie,
        (ORIGIN, &csrf),
        "{}",
        true,
    )
    .await;
    private(&disabled, StatusCode::NO_CONTENT);
    let response = http(
        &app,
        "POST",
        "/api/v1/me/surprises/open",
        &cookie,
        (ORIGIN, &csrf),
        &open,
        true,
    )
    .await;
    private(&response, StatusCode::NOT_FOUND);
    let response = http(
        &app,
        "GET",
        "/api/v1/me/surprises/plan",
        &cookie,
        (ORIGIN, &csrf),
        "",
        true,
    )
    .await;
    private(&response, StatusCode::OK);
    assert_eq!(parse(response).await["state"], "off");

    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
         WHERE user_id={user} AND role='administrator'"
        ),
    );
    let revoked = http(
        &app,
        "GET",
        "/admin/surprises",
        &cookie,
        (ORIGIN, &csrf),
        "",
        false,
    )
    .await;
    private(&revoked, StatusCode::FORBIDDEN);

    clean(&url, user, item_id);
}

fn clean(url: &str, actor: i64, item_id: i64) {
    pg(
        url,
        "UPDATE surprise_runtime_policy SET enabled=false,experiment_enabled=false WHERE singleton",
    );
    pg(
        url,
        &format!("DELETE FROM surprise_admin_events WHERE actor_user_id={actor}"),
    );
    pg(
        url,
        &format!("DELETE FROM surprise_content WHERE id={item_id}"),
    );
    pg(
        url,
        &format!("DELETE FROM account_roles WHERE user_id={actor}"),
    );
    pg(url, &format!("DELETE FROM users WHERE id={actor}"));
}
