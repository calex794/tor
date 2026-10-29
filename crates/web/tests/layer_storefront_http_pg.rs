//! D-05/S031: real NEwBOO browser path against disposable PostgreSQL.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{num::NonZeroU32, process::Command};
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

fn pg(url: &str, sql: &str) -> i64 {
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"),
        "financial HTTP test requires disposable tor_ci"
    );
    let output = Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", sql])
        .output()
        .expect("psql");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

async fn user(accounts: &PgAccounts, role: &str) -> (i64, String) {
    let token = generate_session_token();
    let email = format!(
        "d05-storefront-{role}-{}@example.test",
        token[..12].to_ascii_lowercase()
    );
    let actor = accounts
        .register_with_session(
            &email,
            "Читатель",
            &hash_password("D05NewbooStorefrontVeryStrongPassword2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (actor, token)
}

async fn get(app: &Router, uri: &str, session: Option<&str>) -> Response {
    let mut req = Request::builder().method("GET").uri(uri);
    if let Some(session) = session {
        req = req.header(header::COOKIE, format!("tor-dev-session={session}"));
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn form(
    app: &Router,
    uri: &str,
    fields: &str,
    token: &str,
    csrf: &str,
    origin: Option<&str>,
) -> Response {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(
            header::COOKIE,
            format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        );
    if let Some(origin) = origin {
        req = req.header(header::ORIGIN, origin);
    }
    app.clone()
        .oneshot(req.body(Body::from(fields.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn html(reply: Response) -> String {
    String::from_utf8(to_bytes(reply.into_body(), 128 * 1024).await.unwrap().to_vec()).unwrap()
}

fn input(html: &str, field: &str) -> String {
    let marker = format!("name=\"{field}\" value=\"");
    html.split(&marker)
        .nth(1)
        .expect("server generated hidden purchase field")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci and explicitly enabled Layer storefront"]
async fn d_05_newboo_layer_offer_form_ledger_and_private_library() {
    for name in ["TOR_LAYER_PAGES_ENABLED", "TOR_LAYER_CHECKOUT_ENABLED", "TOR_PAID_CHECKOUT_ENABLED"] {
        assert_eq!(std::env::var(name).as_deref(), Ok("1"), "{name} must be explicitly enabled");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let (author, _) = user(&accounts, "owner").await;
    let (buyer, buyer_token) = user(&accounts, "buyer").await;
    let (_, stranger_token) = user(&accounts, "stranger").await;
    pg(
        &url,
        &format!("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
                 VALUES({buyer},450,'RUB') RETURNING balance_minor"),
    );
    let book = pg(
        &url,
        &format!("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
                 VALUES('ci:d05:storefront:{author}','ru','Книга <script>alert(1)</script>','public',{author})
                 RETURNING id"),
    );
    let hidden = pg(
        &url,
        &format!("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
                 VALUES('ci:d05:storefront:hidden:{author}','ru','SECRET-DRAFT-LAYER','draft',{author})
                 RETURNING id"),
    );
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(PositiveId::new(author).unwrap(), "Пакет <b>автора</b>")
        .await
        .unwrap();
    let first_version = layers
        .add_book(
            PositiveId::new(author).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(book).unwrap(),
        )
        .await
        .unwrap();
    let current = layers
        .add_book(
            PositiveId::new(author).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(hidden).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(current, first_version);
    let offer = layers
        .publish_offer(PositiveId::new(author).unwrap(), PositiveId::new(layer).unwrap(), 125, false)
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog, reader, accounts, false, ORIGIN.to_owned(),
    );
    let path = format!("/layers/{layer}");
    let public = get(&app, &path, None).await;
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(public.headers()[header::CACHE_CONTROL], "no-store");
    let public = html(public).await;
    assert!(public.contains("layers-public-layout"));
    assert!(public.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(public.contains("Пакет &lt;b&gt;автора&lt;/b&gt;"));
    assert!(!public.contains("SECRET-DRAFT-LAYER"));
    assert!(!public.contains("layer-purchase-form"));
    assert!(public.contains("Войти и приобрести доступ"));

    let buyer_page = get(&app, &path, Some(&buyer_token)).await;
    assert_eq!(buyer_page.status(), StatusCode::OK);
    let csrf_cookie = buyer_page
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.strip_prefix("tor-dev-csrf=")
                .map(|x| x.split(';').next().unwrap().to_owned())
        })
        .expect("CSRF cookie");
    let page = html(buyer_page).await;
    let csrf = input(&page, "csrf_token");
    assert_eq!(csrf, csrf_cookie);
    let key = input(&page, "idempotency_key");
    assert_eq!(key.len(), 43);
    let form_body = format!(
        "csrf_token={csrf}&idempotency_key={key}&expected_offer_id={offer}&expected_version_id={current}&expected_price_minor=125&confirm=1"
    );
    assert_eq!(
        form(&app, &format!("{path}/purchase"), &form_body, &buyer_token, &csrf, None)
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    let without_consent = form_body.replace("&confirm=1", "");
    assert_eq!(
        form(&app, &format!("{path}/purchase"), &without_consent, &buyer_token, &csrf, Some(ORIGIN))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(
        form(&app, &format!("{path}/purchase"), &form_body, &stranger_token, &csrf, Some(ORIGIN))
            .await
            .status(),
        StatusCode::CONFLICT,
        "another session cannot charge the intended buyer's wallet",
    );
    assert_eq!(pg(&url, &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")), 450);

    let purchased = form(&app, &format!("{path}/purchase"), &form_body, &buyer_token, &csrf, Some(ORIGIN)).await;
    assert_eq!(purchased.status(), StatusCode::SEE_OTHER);
    assert_eq!(purchased.headers()[header::LOCATION], format!("{path}?purchased=1"));
    assert_eq!(purchased.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(pg(&url, &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")), 325);
    assert_eq!(pg(&url, &format!("SELECT COUNT(*) FROM commerce_layer_terms WHERE user_id={buyer}")), 1);

    assert_eq!(
        form(&app, &format!("{path}/purchase"), &form_body, &buyer_token, &csrf, Some(ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER,
    );
    assert_eq!(pg(&url, &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")), 325);
    assert_eq!(pg(&url, &format!("SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id={buyer} AND kind='layer_purchase'")), 1);

    let terms = get(&app, "/me/library/layers", Some(&buyer_token)).await;
    assert_eq!(terms.status(), StatusCode::OK);
    let terms = html(terms).await;
    assert!(terms.contains("Пакет &lt;b&gt;автора&lt;/b&gt;"));
    assert!(terms.contains("Доступен"));
    assert!(!terms.contains("SECRET-DRAFT-LAYER"));
    let bought_page = html(get(&app, &format!("{path}?purchased=1"), Some(&buyer_token)).await).await;
    assert!(bought_page.contains("Покупка подтверждена"));
    pool.close().await;
}
