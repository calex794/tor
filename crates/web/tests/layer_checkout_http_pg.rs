//! S031: actual session, CSRF, offer drift and wallet purchase via Axum + PG.
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

fn id(raw: i64) -> PositiveId {
    PositiveId::new(raw).unwrap()
}

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never connect to production");
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
            sql,
        ])
        .output()
        .expect("PostgreSQL test client");
    assert!(
        output.status.success(),
        "isolated PG SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

async fn http(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    origin: &str,
    form: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
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

async fn response_body(res: Response) -> String {
    String::from_utf8(
        to_bytes(res.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn field(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap_or_else(|| panic!("form field {name} is missing"))
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

fn buy_form(html: &str, csrf: &str) -> String {
    format!(
        "expected_offer_id={}&expected_version_id={}&expected_price_minor={}&command_key={}&csrf_token={csrf}",
        field(html, "expected_offer_id"),
        field(html, "expected_version_id"),
        field(html, "expected_price_minor"),
        field(html, "command_key"),
    )
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and opt-in Layer wallet checkout"]
async fn d_05_http_layer_checkout_verifies_quote_drift_and_one_debit() {
    assert_eq!(
        std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("TOR_LAYER_CHECKOUT_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").expect("tor_ci fixture");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("D05LayerWalletBrowser2026").unwrap();
    let buyer_token = generate_session_token();
    let owner_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("d05-offer-owner-{nonce}@tor-ci.example"),
            "Layer Author",
            &password,
            &session_token_digest(&owner_token),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let buyer = accounts
        .register_with_session(
            &format!("d05-offer-buyer-{nonce}@tor-ci.example"),
            "Layer Reader",
            &password,
            &session_token_digest(&buyer_token),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},1500,'RUB')"
        ),
    );
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:d05:offer:{nonce}','ru','<script>hidden</script>','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let repo = PgLayers::new(pool.clone());
    let layer = repo.create_layer(id(owner), "Paid Layer").await.unwrap();
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let offer = repo
        .publish_offer(id(owner), id(layer), 250, false)
        .await
        .unwrap();
    let page = format!("/layers/{layer}");
    let checkout = format!("/layers/{layer}/buy");
    let buyer_cookie = format!("tor-dev-session={buyer_token}; tor-dev-csrf={csrf}");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let first = http(&app, "GET", &page, &buyer_cookie, ORIGIN, "").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
    let original = response_body(first).await;
    assert!(original.contains("2,50 ₽"));
    assert!(original.contains("30 дней, только ручное продление"));
    assert!(original.contains("&lt;script&gt;hidden&lt;/script&gt;"));
    assert!(!original.contains("<script>hidden</script>"));
    assert_eq!(field(&original, "expected_offer_id"), offer.to_string());
    assert_eq!(field(&original, "expected_version_id"), version.to_string());
    assert_eq!(field(&original, "command_key").len(), 43);
    let stale_version_form = buy_form(&original, &csrf);
    assert_eq!(
        http(&app, "POST", &checkout, "", ORIGIN, &stale_version_form)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            "https://evil.invalid",
            &stale_version_form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            ORIGIN,
            &stale_version_form.replace(&format!("csrf_token={csrf}"), "csrf_token=invalid"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let second: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:d05:offer:second:{nonce}','ru','Second book','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    repo.add_book(id(owner), id(layer), id(second))
        .await
        .unwrap();
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            ORIGIN,
            &stale_version_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "changed composition refuses the old quote without wallet debit"
    );
    let after_version =
        response_body(http(&app, "GET", &page, &buyer_cookie, ORIGIN, "").await).await;
    assert!(after_version.contains("Second book"));
    let stale_price_form = buy_form(&after_version, &csrf);
    repo.publish_offer(id(owner), id(layer), 300, false)
        .await
        .unwrap();
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            ORIGIN,
            &stale_price_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "changed price refuses the old offer and does not take money"
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &owner_cookie,
            ORIGIN,
            &stale_price_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "even the owner cannot reuse a superseded quote"
    );
    let current = response_body(http(&app, "GET", &page, &buyer_cookie, ORIGIN, "").await).await;
    assert!(current.contains("3,00 ₽"));
    let form = buy_form(&current, &csrf);
    assert_eq!(
        http(&app, "POST", &checkout, &owner_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "selling Layer to its owner is forbidden"
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            ORIGIN,
            &form.replace("command_key=", "command_key=invalid"),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let bought = http(&app, "POST", &checkout, &buyer_cookie, ORIGIN, &form).await;
    assert_eq!(bought.status(), StatusCode::OK);
    assert_eq!(bought.headers()[header::CACHE_CONTROL], "no-store");
    let bought_html = response_body(bought).await;
    assert!(bought_html.contains("Баланс кошелька после оплаты: 1200 коп."));
    let replay =
        response_body(http(&app, "POST", &checkout, &buyer_cookie, ORIGIN, &form).await).await;
    assert!(replay.contains("Повторное списание не выполнялось."));
    assert_eq!(
        http(
            &app,
            "POST",
            &checkout,
            &buyer_cookie,
            ORIGIN,
            &form.replace("expected_price_minor=300", "expected_price_minor=301"),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let entries: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM reader_wallet_entries
             WHERE user_id={buyer} AND kind='layer_purchase'"
        ),
    )
    .parse()
    .unwrap();
    let terms: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM commerce_layer_terms
             WHERE user_id={buyer} AND layer_id={layer}"
        ),
    )
    .parse()
    .unwrap();
    let balance: i64 = pg(
        &url,
        &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"),
    )
    .parse()
    .unwrap();
    assert_eq!((entries, terms, balance), (1, 1, 1200));
    pool.close().await;
}
