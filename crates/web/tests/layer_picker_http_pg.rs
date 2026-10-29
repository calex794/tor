//! D-05: owner-only Layer selection via real Axum sessions + PostgreSQL.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_commerce::repositories::layers::PgLayers;
use tor_contracts::paid_delivery::PositiveId;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never query production");
    let output = Command::new("psql")
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
        .output()
        .expect("PostgreSQL client");
    assert!(
        output.status.success(),
        "isolated PG fixture error: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    app: &Router,
    method: &str,
    url: &str,
    cookies: &str,
    origin: &str,
    form: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(url)
                .method(method)
                .header(header::COOKIE, cookies)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and TOR_LAYER_MANAGEMENT_ENABLED=1"]
async fn d_05_http_owner_only_layer_picker_filters_team_and_rechecks_stale_form() {
    assert_eq!(
        std::env::var("TOR_LAYER_MANAGEMENT_ENABLED").as_deref(),
        Ok("1"),
        "this test must enable only the isolated Layer picker"
    );
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test fixture");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner_session = generate_session_token();
    let outsider_session = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("D05PickerOnlyOwnerPassword2026").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("d05-picker-owner-{nonce}@tor-ci.example"),
            "Owner",
            &password,
            &session_token_digest(&owner_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let collaborator = accounts
        .register_with_session(
            &format!("d05-picker-collaborator-{nonce}@tor-ci.example"),
            "Collaborator",
            &password,
            &session_token_digest(&outsider_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let solo: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:d05:picker:solo:{nonce}','ru','Solo <script>','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let team: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:d05:picker:team:{nonce}','ru','Team book','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO book_members(book_id,user_id,role,is_active)
             VALUES({team},{collaborator},'translator',TRUE)"
        ),
    );
    let layer = PgLayers::new(pool.clone())
        .create_layer(PositiveId::new(owner).unwrap(), "D05 browser picker")
        .await
        .unwrap();
    let page = format!("/me/layers/{layer}/books");
    let owner_cookie = format!("tor-dev-session={owner_session}; tor-dev-csrf={csrf}");
    let other_cookie = format!("tor-dev-session={outsider_session}; tor-dev-csrf={csrf}");
    let allowed_form = format!("book_id={solo}&csrf_token={csrf}");
    let team_form = format!("book_id={team}&csrf_token={csrf}");

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    // The owner can discover the picker from their existing author
    // dashboard; a foreign actor never receives that private Layer link.
    let owner_dashboard = body(
        request(&app, "GET", "/me/books", &owner_cookie, ORIGIN, "").await,
    )
    .await;
    assert!(owner_dashboard.contains("my-layers-heading"));
    assert!(owner_dashboard.contains(&format!("/me/layers/{layer}/books")));
    assert!(owner_dashboard.contains("D05 browser picker"));
    let foreign_dashboard = body(
        request(&app, "GET", "/me/books", &other_cookie, ORIGIN, "").await,
    )
    .await;
    assert!(!foreign_dashboard.contains(&format!("/me/layers/{layer}/books")));
    assert_eq!(
        request(&app, "GET", &page, "", ORIGIN, "").await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(&app, "GET", &page, &other_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let owner_page = request(&app, "GET", &page, &owner_cookie, ORIGIN, "").await;
    assert_eq!(owner_page.status(), StatusCode::OK);
    assert_eq!(owner_page.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(owner_page).await;
    assert!(html.contains("Solo &lt;script&gt;"));
    assert!(!html.contains("Solo <script>"));
    assert!(!html.contains("Team book"));
    assert!(html.contains(&format!("name=\"book_id\" value=\"{solo}\"")));
    assert!(!html.contains(&format!("name=\"book_id\" value=\"{team}\"")));

    assert_eq!(
        request(
            &app,
            "POST",
            &page,
            &owner_cookie,
            "https://evil.invalid",
            &allowed_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &page,
            &owner_cookie,
            ORIGIN,
            &format!("book_id={solo}&csrf_token=invalid")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &team_form)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "team Books cannot bypass the UI with handcrafted POST"
    );
    pg(
        &url,
        &format!("UPDATE book_members SET is_active=FALSE WHERE book_id={team} AND user_id={collaborator}"),
    );
    let now = body(request(&app, "GET", &page, &owner_cookie, ORIGIN, "").await).await;
    assert!(now.contains("Team book"));
    pg(
        &url,
        &format!("UPDATE book_members SET is_active=TRUE WHERE book_id={team} AND user_id={collaborator}"),
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &team_form)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "stale picker must not bypass the fresh locked D check"
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &allowed_form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let updated = body(request(&app, "GET", &page, &owner_cookie, ORIGIN, "").await).await;
    assert!(!updated.contains("Solo &lt;script&gt;"));
    let versions: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM commerce_layer_versions WHERE layer_id={layer}"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        versions, 2,
        "exactly one Book addition, one immutable revision"
    );
    pool.close().await;
}
