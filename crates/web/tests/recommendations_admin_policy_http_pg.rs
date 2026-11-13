//! REC-ADMIN-01: real role/CSRF/CAS/audit acceptance against disposable tor_ci.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
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
const PATH: &str = "/admin/recommendations";

fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let out = Command::new("psql")
        .env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000")
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "-c", statement])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn encoded(fields: &[(&str, String)]) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
    url.query_pairs_mut()
        .extend_pairs(fields.iter().map(|(k, v)| (*k, v)));
    url.query().unwrap_or_default().to_owned()
}

fn save_form(csrf: &str, revision: i64, ratios: [i32; 5]) -> String {
    encoded(&[
        ("csrf_token", csrf.to_owned()),
        ("action", "save".into()),
        ("expected_revision", revision.to_string()),
        ("author_pct", ratios[0].to_string()),
        ("teammate_pct", ratios[1].to_string()),
        ("other_authors_pct", ratios[2].to_string()),
        ("no_team_author_pct", ratios[3].to_string()),
        ("no_team_other_authors_pct", ratios[4].to_string()),
    ])
}

async fn request(
    site: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    form: &str,
) -> Response {
    site.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .method(method)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with policy migration"]
async fn policy_html_administrator_csrf_cas_audit_and_rollback() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("RecommendationAdminHttpPassword2026").unwrap();
    let admin_token = generate_session_token();
    let reader_token = generate_session_token();
    let csrf = generate_session_token();
    let admin = accounts
        .register_with_session(
            &format!("rec-admin-{stamp}@example.invalid"),
            "Recommendation admin",
            &password,
            &session_token_digest(&admin_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("rec-reader-{stamp}@example.invalid"),
            "Recommendation reader",
            &password,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let site = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let reader_cookie = format!("tor-dev-session={reader_token}; tor-dev-csrf={csrf}");
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (reader_cookie.as_str(), StatusCode::FORBIDDEN),
    ] {
        for method in ["GET", "POST"] {
            let response = request(
                &site,
                method,
                PATH,
                cookie,
                ORIGIN,
                &format!("action=defaults&expected_revision=1&csrf_token={csrf}"),
            )
            .await;
            assert_eq!(response.status(), status);
        }
    }

    let initial: i64 = pg(
        &url,
        "SELECT revision FROM rec_shelf_share_policy WHERE singleton",
    )
    .parse()
    .unwrap();
    let initial_audit: i64 = pg(
        &url,
        "SELECT count(*) FROM rec_shelf_share_policy_audit",
    )
    .parse()
    .unwrap();
    let page = request(&site, "GET", PATH, &admin_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(page.headers()["x-robots-tag"], "noindex, nofollow");
    let markup = html(page).await;
    assert!(markup.contains("data-testid=\"rec-policy-form\""));
    assert!(markup.contains("name=\"expected_revision\""));
    assert!(markup.contains("name=\"no_team_other_authors_pct\""));
    assert!(markup.contains("Другие авторы"));
    assert!(!markup.contains("data-flow-proof="));

    let good = save_form(&csrf, initial, [40, 30, 30, 60, 40]);
    for (origin, body, status) in [
        ("https://evil.invalid", good.clone(), StatusCode::FORBIDDEN),
        (ORIGIN, good.replace(&csrf, "wrong"), StatusCode::FORBIDDEN),
        (
            ORIGIN,
            save_form(&csrf, initial, [50, 40, 20, 60, 40]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ORIGIN,
            save_form(&csrf, initial, [-1, 51, 50, 60, 40]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = request(&site, "POST", PATH, &admin_cookie, origin, &body).await;
        assert_eq!(response.status(), status);
    }
    assert_eq!(
        pg(&url, "SELECT revision FROM rec_shelf_share_policy WHERE singleton"),
        initial.to_string()
    );
    let good = save_form(&csrf, initial, [40, 30, 30, 60, 40]);
    let changed = request(&site, "POST", PATH, &admin_cookie, ORIGIN, &good).await;
    assert_eq!(changed.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        changed.headers()[header::LOCATION],
        format!("{PATH}?changed={}", initial + 1)
    );
    assert_eq!(
        pg(
            &url,
            "SELECT author_pct||'/'||teammate_pct||'/'||other_authors_pct||'|'||
                    no_team_author_pct||'/'||no_team_other_authors_pct
             FROM rec_shelf_share_policy WHERE singleton",
        ),
        "40/30/30|60/40"
    );
    assert_eq!(
        request(&site, "POST", PATH, &admin_cookie, ORIGIN, &good)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT actor_user_id FROM rec_shelf_share_policy_audit WHERE revision={}",
                initial + 1
            ),
        ),
        admin.to_string()
    );
    let stale = save_form(&csrf, initial, [50, 40, 10, 70, 30]);
    assert_eq!(
        request(&site, "POST", PATH, &admin_cookie, ORIGIN, &stale)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let restore = encoded(&[
        ("csrf_token", csrf.clone()),
        ("action", "restore".into()),
        ("target_revision", "1".into()),
        ("expected_revision", (initial + 1).to_string()),
    ]);
    let restored = request(&site, "POST", PATH, &admin_cookie, ORIGIN, &restore).await;
    assert_eq!(restored.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        pg(
            &url,
            "SELECT author_pct||'/'||teammate_pct||'/'||other_authors_pct
             FROM rec_shelf_share_policy WHERE singleton",
        ),
        "50/40/10"
    );
    assert_eq!(
        pg(&url, "SELECT count(*) FROM rec_shelf_share_policy_audit")
            .parse::<i64>()
            .unwrap(),
        initial_audit + 2
    );

    let response = request(
        &site,
        "GET",
        &format!("{PATH}?book=1&chapter=2"),
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preview = html(response).await;
    assert!(preview.contains("data-testid=\"rec-preview-counts\""));
    assert!(!preview.contains("data-flow-proof="));
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={admin} AND role='administrator'"
        ),
    );
    assert_eq!(
        request(&site, "GET", PATH, &admin_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            PATH,
            &admin_cookie,
            ORIGIN,
            &save_form(&csrf, initial + 2, [50, 40, 10, 70, 30])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    pg(&url, &format!("DELETE FROM account_roles WHERE user_id={admin}"));
    pg(
        &url,
        &format!("DELETE FROM sessions WHERE user_id IN({admin},{reader})"),
    );
    pool.close().await;
}
