//! S3-ADMIN HTTP acceptance: live roles, private form, CSRF and no external writes.
//! Deliberately never contact the fixed Yandex endpoint from CI.
use super::{pg, ORIGIN};
use reqwest::{Client, StatusCode};

const ROUTE: &str = "/odmen/storage/s3";
const VALID_SHAPE: &str =
    "bucket=ci-disposable&access_key_id=CI_TEST_ACCESS_0000&secret_access_key=CI_TEST_SECRET_000000000000";

fn endpoint() -> String {
    format!("{ORIGIN}{ROUTE}")
}

async fn get(client: &Client, cookie: &str) -> reqwest::Response {
    client
        .get(endpoint())
        .header("cookie", cookie)
        .send()
        .await
        .expect("local admin connection form")
}

async fn post(client: &Client, cookie: &str, origin: &str, body: &str) -> StatusCode {
    client
        .post(endpoint())
        .header("cookie", cookie)
        .header("origin", origin)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.to_owned())
        .send()
        .await
        .expect("local admin connection POST")
        .status()
}

pub(super) async fn check_connection_form(
    client: &Client,
    db: &str,
    admin_session: &str,
    reader_session: &str,
) {
    let admin = format!("tor-dev-session={admin_session}");
    let reader = format!("tor-dev-session={reader_session}");
    let before = pg(
        db,
        "SELECT (SELECT count(*) FROM admin_s3_profiles) +          (SELECT count(*) FROM admin_s3_profile_audit)",
    );
    let denied_form = format!("{VALID_SHAPE}&csrf_token=invalid");
    assert_eq!(get(client, "").await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(get(client, &reader).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        post(client, "", ORIGIN, &denied_form).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(client, &reader, ORIGIN, &denied_form).await,
        StatusCode::FORBIDDEN
    );

    let menu = client
        .get(format!("{ORIGIN}/odmen"))
        .header("cookie", &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(menu.status(), StatusCode::OK);
    assert!(menu.text().await.unwrap().contains(ROUTE));

    let form = get(client, &admin).await;
    assert_eq!(form.status(), StatusCode::OK);
    assert_eq!(form.headers()["cache-control"], "no-store");
    let csrf_cookie = form
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .starts_with("tor-dev-csrf=")
                .then(|| value.split(';').next().unwrap().to_owned())
        })
        .expect("admin CSRF cookie");
    let html = form.text().await.unwrap();
    assert!(html.contains(&format!("action=\"{ROUTE}\"")));
    assert!(html.contains("type=\"password\" name=\"access_key_id\""));
    assert!(html.contains("type=\"password\" name=\"secret_access_key\""));
    assert!(!html.contains("CI_TEST_SECRET_000000000000"));
    let csrf = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("admin CSRF hidden input");
    let cookies = format!("{admin}; {csrf_cookie}");
    let payload = format!("{VALID_SHAPE}&csrf_token={csrf}");
    // Every POST below is rejected before the network call.
    assert_eq!(
        post(client, &admin, ORIGIN, &payload).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(client, &cookies, "https://invalid.example", &payload).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(client, &cookies, ORIGIN, &denied_form).await,
        StatusCode::FORBIDDEN
    );
    let invalid_bucket = payload.replace("bucket=ci-disposable", "bucket=ab");
    assert_eq!(
        post(client, &cookies, ORIGIN, &invalid_bucket).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        pg(
            db,
            "SELECT (SELECT count(*) FROM admin_s3_profiles) +              (SELECT count(*) FROM admin_s3_profile_audit)",
        ),
        before,
        "rejected connection forms must not create a profile or audit write"
    );
}

pub(super) async fn check_revoked(client: &Client, session: &str) {
    let cookie = format!("tor-dev-session={session}");
    let body = format!("{VALID_SHAPE}&csrf_token=invalid");
    assert_eq!(get(client, &cookie).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        post(client, &cookie, ORIGIN, &body).await,
        StatusCode::FORBIDDEN
    );
}
