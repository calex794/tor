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
    let owner_dashboard =
        body(request(&app, "GET", "/me/books", &owner_cookie, ORIGIN, "").await).await;
    assert!(owner_dashboard.contains("my-layers-heading"));
    assert!(owner_dashboard.contains(&format!("/me/layers/{layer}/books")));
    assert!(owner_dashboard.contains("D05 browser picker"));
    assert!(owner_dashboard.contains("href=\"/me/layers\""));
    // Direct owner index must require a session and never list another owner.
    assert_eq!(
        request(&app, "GET", "/me/layers", "", ORIGIN, "")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let list_response = request(&app, "GET", "/me/layers", &owner_cookie, ORIGIN, "").await;
    assert_eq!(list_response.status(), StatusCode::OK);
    assert_eq!(list_response.headers()[header::CACHE_CONTROL], "no-store");
    let own_layers = body(list_response).await;
    assert!(own_layers.contains("Мои слои"));
    assert!(own_layers.contains("href=\"/me/layers/new\""));
    assert!(own_layers.contains(&format!("/me/layers/{layer}/books")));
    assert!(own_layers.contains("D05 browser picker"));
    let foreign_layers =
        body(request(&app, "GET", "/me/layers", &other_cookie, ORIGIN, "").await).await;
    assert!(!foreign_layers.contains("D05 browser picker"));
    assert!(!foreign_layers.contains(&format!("/me/layers/{layer}/books")));
    let foreign_dashboard =
        body(request(&app, "GET", "/me/books", &other_cookie, ORIGIN, "").await).await;
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
    assert!(updated.contains("Solo &lt;script&gt;"));
    assert!(updated.contains("name=\"operation\" value=\"remove\""));
    let current_version: i64 = pg(
        &url,
        &format!("SELECT current_version_id FROM commerce_layers WHERE id={layer}"),
    )
    .parse()
    .unwrap();
    assert!(updated.contains(&format!(
        "name=\"expected_version_id\" value=\"{current_version}\""
    )));
    // Owner price publication is separate from wallet checkout, protected
    // by the same actor session, Origin and CSRF as composition changes.
    let offer_path = format!("/me/layers/{layer}/offer");
    assert!(updated.contains("name=\"price_rub\""));
    assert!(updated.contains("name=\"expected_offer_id\" value=\"0\""));
    let price_form = format!(
        "expected_version_id={current_version}&expected_offer_id=0&price_rub=149%2C50&term_kind=prepaid_30d&csrf_token={csrf}"
    );
    let old_version_form = price_form.replace(
        &format!("expected_version_id={current_version}"),
        "expected_version_id=1",
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &other_cookie,
            ORIGIN,
            &price_form
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "a reader may not publish the owner's price"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            "https://evil.invalid",
            &price_form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            ORIGIN,
            &price_form.replace(&format!("csrf_token={csrf}"), "csrf_token=invalid"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            ORIGIN,
            &price_form.replace("price_rub=149%2C50", "price_rub=149%2C555"),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            ORIGIN,
            &old_version_form,
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            ORIGIN,
            &price_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &offer_path,
            &owner_cookie,
            ORIGIN,
            &price_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "reposting the original price form must not mint an offer revision"
    );
    let offer_count: i64 = pg(
        &url,
        &format!("SELECT COUNT(*) FROM commerce_layer_offers WHERE layer_id={layer}"),
    )
    .parse()
    .unwrap();
    assert_eq!(offer_count, 1);
    let current_offer: i64 = pg(
        &url,
        &format!("SELECT current_offer_id FROM commerce_layers WHERE id={layer}"),
    )
    .parse()
    .unwrap();
    let priced_page = body(request(&app, "GET", &page, &owner_cookie, ORIGIN, "").await).await;
    assert!(priced_page.contains("value=\"149,50\""));
    assert!(priced_page.contains(&format!(
        "name=\"expected_offer_id\" value=\"{current_offer}\""
    )));
    let original_version: i64 = pg(
        &url,
        &format!("SELECT id FROM commerce_layer_versions WHERE layer_id={layer} AND version_no=1"),
    )
    .parse()
    .unwrap();
    let remove = format!(
        "operation=remove&book_id={solo}&expected_version_id={current_version}&csrf_token={csrf}"
    );
    let stale = format!(
        "operation=remove&book_id={solo}&expected_version_id={original_version}&csrf_token={csrf}"
    );
    assert_eq!(
        request(&app, "POST", &page, &other_cookie, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &stale)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &page,
            &owner_cookie,
            "https://evil.invalid",
            &remove
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
            &format!(
                "operation=remove&book_id={solo}&expected_version_id={current_version}&csrf_token=invalid"
            ),
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
            &format!("operation=remove&book_id={solo}&csrf_token={csrf}"),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(&app, "POST", &page, &owner_cookie, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::CONFLICT,
        "repeated removal cannot create another composition revision"
    );
    let snapshot_count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM commerce_layer_version_books
             WHERE layer_version_id={current_version} AND book_id={solo}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(snapshot_count, 1, "old composition is not rewritten");
    let current_count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM commerce_layer_version_books
             WHERE layer_version_id=(SELECT current_version_id FROM commerce_layers WHERE id={layer})
               AND book_id={solo}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(current_count, 0);
    let after = body(request(&app, "GET", &page, &owner_cookie, ORIGIN, "").await).await;
    assert!(after.contains("Solo &lt;script&gt;"));
    assert!(!after.contains("name=\"operation\" value=\"remove\""));
    let versions: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM commerce_layer_versions WHERE layer_id={layer}"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        versions, 3,
        "add and remove produce two immutable revisions"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and TOR_LAYER_MANAGEMENT_ENABLED=1"]
async fn d_05_http_create_layer_is_csrf_protected_and_replay_safe() {
    assert_eq!(
        std::env::var("TOR_LAYER_MANAGEMENT_ENABLED").as_deref(),
        Ok("1")
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
    let session = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("D05CreateLayerPassword2026").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("d05-create-owner-{nonce}@tor-ci.example"),
            "Layer owner",
            &password,
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let route = "/me/layers/new";
    assert_eq!(
        request(&app, "GET", route, "", ORIGIN, "").await.status(),
        StatusCode::SEE_OTHER
    );
    let page = request(&app, "GET", route, &cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(page).await;
    assert!(html.contains("action=\"/me/layers/new\""));
    assert!(html.contains("name=\"title\""));
    assert!(!html.contains("name=\"user_id\""));
    let key = html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(key.len(), 43);
    let form = format!("title=New+Layer&command_key={key}&csrf_token={csrf}");
    let modified = format!("title=Changed+Layer&command_key={key}&csrf_token={csrf}");
    assert_eq!(
        request(&app, "POST", route, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "POST", route, &cookie, "https://evil.invalid", &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            route,
            &cookie,
            ORIGIN,
            &format!("title=New+Layer&command_key={key}&csrf_token=invalid")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            route,
            &cookie,
            ORIGIN,
            &format!("title=New+Layer&command_key=invalid&csrf_token={csrf}")
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let created = request(&app, "POST", route, &cookie, ORIGIN, &form).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let destination = created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(destination.starts_with("/me/layers/"));
    assert!(destination.ends_with("/books"));
    let replay = request(&app, "POST", route, &cookie, ORIGIN, &form).await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], destination);
    assert_eq!(
        request(&app, "POST", route, &cookie, ORIGIN, &modified)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM commerce_layers
             WHERE owner_user_id={owner} AND create_command_key='{key}'"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(count, 1);
    let id: i64 = destination
        .trim_start_matches("/me/layers/")
        .trim_end_matches("/books")
        .parse()
        .unwrap();
    let versions: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM commerce_layer_versions WHERE layer_id={id}"),
    )
    .parse()
    .unwrap();
    let books: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM commerce_layer_version_books
             WHERE layer_version_id IN
                (SELECT id FROM commerce_layer_versions WHERE layer_id={id})"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!((versions, books), (1, 0));
    let dashboard = body(request(&app, "GET", "/me/books", &cookie, ORIGIN, "").await).await;
    assert!(dashboard.contains(&destination));
    assert!(dashboard.contains("New Layer"));
    pool.close().await;
}
