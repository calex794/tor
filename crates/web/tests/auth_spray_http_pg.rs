//! AB-02: exercise real HTTP preflight with a PostgreSQL-triggered spray lock.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::{
        auth_observation::AuthObservationSigner, profile::ProfileUpdate, security::hash_password,
    },
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PEER: [u8; 4] = [198, 51, 100, 94];

fn request(method: &str, path: &str, form: &str, cookie: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .body(Body::from(form.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((PEER, 40002))));
    req
}
async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.unwrap()
}
async fn text(reply: Response) -> String {
    let body = to_bytes(reply.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}
fn login_form(email: &str, csrf: &str) -> String {
    let encoded = email.replace('@', "%40");
    format!("email={encoded}&password=StrongPasswordForAB02%21&csrf_token={csrf}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci; no production"]
async fn a09_http_spray_guard_responds_without_account_enumeration_or_session() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test-only database");
    assert!(url.contains("127.0.0.1") && url.ends_with("/tor_ci"));
    // This executable has one test; these are process-local test settings.
    std::env::set_var("TOR_AUTH_SPRAY_MODE", "enforce");
    std::env::set_var("TOR_AUTH_SPRAY_KEY", "a9".repeat(32));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let reader = PgReader::new(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let known = format!("spray-known-{nonce}@example.invalid");
    let unknown = format!("spray-unknown-{nonce}@example.invalid");
    let hash = hash_password("StrongPasswordForAB02!").unwrap();
    let actor = accounts
        .insert_user(&known, "Spray Reader", &hash)
        .await
        .unwrap();
    let handle = format!("spray_{nonce:x}");
    let missing_handle = format!("missing_{nonce:x}");
    let mut claim = ProfileUpdate {
        display_name: None,
        handle: Some(handle.clone()),
        adult_enabled: None,
        expected_revision: 0,
    };
    claim.validate().unwrap();
    assert_eq!(accounts.update_profile(actor, &claim).await.unwrap(), 1);
    let signer = AuthObservationSigner::from_hex(&"a9".repeat(32)).unwrap();
    let (ip, net) = signer.source(SocketAddr::from((PEER, 40002)).ip());
    for i in 0..20 {
        let victim = signer.target(&format!("spray-target-{nonce}-{}@example.invalid", i % 12));
        accounts
            .record_spray_failure(&ip, &net, &victim, true)
            .await
            .unwrap();
    }
    assert!(accounts.spray_retry_after(&ip).await.unwrap().is_some());
    let login_page = send(&app, request("GET", "/login", "", "", ORIGIN)).await;
    assert_eq!(login_page.status(), StatusCode::OK);
    let cookie = login_page
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let text = v.to_str().ok()?;
            text.starts_with("tor-dev-csrf=")
                .then(|| text.split(';').next().unwrap_or_default().to_owned())
        })
        .unwrap();
    let html = text(login_page).await;
    let csrf = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();

    // Invalid origin/CSRF is still rejected before any spray lookup.
    let rejected = send(
        &app,
        request(
            "POST",
            "/login",
            &login_form(&known, csrf),
            &cookie,
            "https://evil.example",
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    for email in [&known, &unknown, &handle, &missing_handle] {
        let reply = send(
            &app,
            request("POST", "/login", &login_form(email, csrf), &cookie, ORIGIN),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
        assert!(reply.headers().get(header::SET_COOKIE).is_none());
        let retry: u32 = reply.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=90).contains(&retry));
        let body = text(reply).await;
        assert!(!body.contains(email));
        assert!(!body.contains("password_hash"));
    }

    // An operator can turn off enforcement without changing a password or
    // erasing the source counter; original per-IP/account limits still apply.
    std::env::set_var("TOR_AUTH_SPRAY_MODE", "off");
    let resumed = send(
        &app,
        request("POST", "/login", &login_form(&known, csrf), &cookie, ORIGIN),
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::SEE_OTHER);
    assert!(resumed
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap_or("").starts_with("tor-dev-session=")));
    // Upper-case handle login resolves the same active account and clears both
    // the submitted handle guard and the original canonical email budget.
    let alias = send(
        &app,
        request(
            "POST",
            "/login",
            &login_form(&handle.to_uppercase(), csrf),
            &cookie,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(alias.status(), StatusCode::SEE_OTHER);
    assert!(alias
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap_or("").starts_with("tor-dev-session=")));

    // Eight wrong passwords total, not eight per alias. Varying the case
    // of the handle must not create another bucket.
    for i in 0..8 {
        let identity = if i % 2 == 0 {
            known.clone()
        } else {
            handle.to_uppercase()
        };
        let wrong =
            login_form(&identity, csrf).replace("StrongPasswordForAB02%21", "WrongPassword2026");
        let denied = send(
            &app,
            request("POST", "/login", &wrong, &cookie, ORIGIN),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(denied.headers().get(header::SET_COOKIE).is_none());
    }
    for identity in [&known, &handle] {
        let denied = send(
            &app,
            request("POST", "/login", &login_form(identity, csrf), &cookie, ORIGIN),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(denied.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(denied.headers()[header::RETRY_AFTER], "600");
        assert!(!text(denied).await.contains(&known));
    }
    std::env::remove_var("TOR_AUTH_SPRAY_MODE");
    std::env::remove_var("TOR_AUTH_SPRAY_KEY");
}
