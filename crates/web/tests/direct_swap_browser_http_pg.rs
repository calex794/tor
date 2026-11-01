//! D-06: exercise the actual two-step swap browser on disposable PostgreSQL.
//! Do not enable this route or connect to a retained database in these tests.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{num::NonZeroU32, process::Command, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_commerce::{
    domain::money::{
        CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PATH: &str = "/me/refunds/swap";

fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "financial tests require disposable tor_ci");
    let result = Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", statement])
        .output()
        .expect("psql available in PostgreSQL integration CI");
    assert!(
        result.status.success(),
        "isolated fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn send(app: &Router, method: &str, cookie: &str, origin: &str, form: &str) -> Response {
    let req = Request::builder()
        .uri(PATH)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn html(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn hidden(markup: &str, name: &str) -> String {
    let prefix = format!("name=\"{name}\" value=\"");
    markup
        .split_once(&prefix)
        .expect("server must issue exact confirmation field")
        .1
        .split_once('"')
        .expect("confirmation field has closing quote")
        .0
        .to_owned()
}

fn app(url: &str, accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    assert!(url.ends_with("/tor_ci"));
    tor_web::router_with_postgres_reading_auth(
        catalog, reader, accounts, false, ORIGIN.to_owned(),
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci; explicit browser flags disabled"]
async fn d_06_swap_http_disabled_by_default() {
    assert_ne!(std::env::var("TOR_DIRECT_SWAP_BROWSER_ENABLED").as_deref(), Ok("1"));
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(&url, accounts, catalog);
    assert_eq!(
        send(&site, "GET", "", ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&site, "POST", "", ORIGIN, "step=commit").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with both browser flags enabled"]
async fn d_06_swap_http_preview_csrf_actor_stale_quote_commit_and_replay() {
    assert_eq!(std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(), Ok("1"));
    assert_eq!(std::env::var("TOR_DIRECT_SWAP_BROWSER_ENABLED").as_deref(), Ok("1"));
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "never write to persistent DB");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let buyer = accounts
        .register_with_session(
            &format!("d06-browser-{nonce}@example.invalid"),
            "Swap buyer",
            &hash_password("SwapBrowserTestPassword2026").unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let other_session = generate_session_token();
    let other = accounts
        .register_with_session(
            &format!("d06-browser-other-{nonce}@example.invalid"),
            "Other reader",
            &hash_password("SwapBrowserOtherPassword2026").unwrap(),
            &session_token_digest(&other_session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(buyer, other);
    pg(&url, &format!(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES({buyer},1000,'RUB')"
    ));
    let owner: i64 = pg(&url, &format!(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d06-browser-owner-{nonce}@example.invalid','Author','$argon2id$ci')
         RETURNING id"
    )).parse().unwrap();
    let source_book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d06:browser:source:{nonce}','ru','Исходная книга','public',{owner})
         RETURNING id"
    )).parse().unwrap();
    let target_book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d06:browser:target:{nonce}','ru','Новая книга','public',{owner})
         RETURNING id"
    )).parse().unwrap();
    let legal = format!("ci:d06:browser:legal:{nonce}");
    let terms = format!("ci:d06:browser:terms:{nonce}");
    pg(&url, &format!(
        "INSERT INTO finance_direct_refund_policy_catalog
         (version,approval_reference,approved_by_user_id,approved_at,effective_at,window_seconds)
         VALUES('{legal}','ci:d06:browser:approval:{nonce}',{owner},
                now()-INTERVAL '2 days',now()-INTERVAL '1 day',86400)"
    ));
    for book in [source_book, target_book] {
        pg(&url, &format!(
            "INSERT INTO finance_revenue_terms
             (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
             VALUES({book},'{terms}',2000,'{legal}',86400)"
        ));
        pg(&url, &format!(
            "INSERT INTO finance_revenue_term_states(book_id,policy_version)
             VALUES({book},'{terms}')"
        ));
    }
    let mut chapters = Vec::new();
    for (book,price) in [(source_book,300),(target_book,450)] {
        let chapter: i64 = pg(&url, &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Платная глава','public','paid',{price}) RETURNING id"
        )).parse().unwrap();
        let version: i64 = pg(&url, &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Закрытая глава') RETURNING id"
        )).parse().unwrap();
        pg(&url, &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{version})"
        ));
        chapters.push((chapter,version));
    }
    let (source_ch,source_version) = chapters[0];
    let (target_ch,target_version) = chapters[1];
    let key = IdempotencyKey::parse(&format!("{}8", "W".repeat(42))).unwrap();
    let purchase = PurchaseCommand::new(
        PositiveId::new(buyer).unwrap(),
        WalletId::new(buyer).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(source_ch).unwrap(),
            version_id: PositiveId::new(source_version).unwrap(),
        },
        MoneyMinor::new(300).unwrap(),
        key,
    ).unwrap();
    let source_purchase = PgCheckout::new(pool.clone()).with_creator_posting(true)
        .checkout_once(&purchase).await.unwrap().purchase_id.get();
    let site = app(&url, accounts, catalog);
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let foreign_cookie = format!("tor-dev-session={other_session}; tor-dev-csrf={csrf}");
    assert_eq!(send(&site, "GET", "", ORIGIN, "").await.status(), StatusCode::SEE_OTHER);
    let options = send(&site, "GET", &cookie, ORIGIN, "").await;
    assert_eq!(options.status(), StatusCode::OK);
    assert_eq!(options.headers()[header::CACHE_CONTROL], "no-store");
    let options_html = html(options).await;
    assert!(options_html.contains(&format!("value=\"{source_purchase}:{target_ch}\"")));
    assert!(!html(send(&site, "GET", &foreign_cookie, ORIGIN, "").await)
        .await.contains(&format!("value=\"{source_purchase}:{target_ch}\"")));
    let choose = format!("csrf_token={csrf}&step=preview&choice={source_purchase}:{target_ch}");
    assert_eq!(send(&site, "POST", &cookie, "http://evil.invalid", &choose)
        .await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&site, "POST", &cookie, ORIGIN, "csrf_token=invalid&step=preview&choice=1:2")
        .await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&site, "POST", "", ORIGIN, &choose).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&site, "POST", &cookie, ORIGIN, &format!("{choose}&user_id={other}"))
        .await.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(send(&site, "POST", &foreign_cookie, ORIGIN, &choose)
        .await.status(), StatusCode::CONFLICT);
    let preview = send(&site, "POST", &cookie, ORIGIN, &choose).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview_html = html(preview).await;
    assert!(preview_html.contains("Доплата"));
    assert_eq!(hidden(&preview_html,"version"), target_version.to_string());
    assert_eq!(hidden(&preview_html,"source_price"), "300");
    assert_eq!(hidden(&preview_html,"target_price"), "450");
    let submit = format!(
        "csrf_token={csrf}&step=commit&source={source_purchase}&target={target_ch}&version={target_version}&source_price=300&target_price=450&command_key={}",
        hidden(&preview_html, "command_key")
    );
    assert_eq!(
        send(&site, "POST", &foreign_cookie, ORIGIN, &submit).await.status(),
        StatusCode::NOT_FOUND,
        "another account cannot settle the quoted purchase"
    );
    pg(&url, &format!("UPDATE chapters SET price_amount=451 WHERE id={target_ch}"));
    assert_eq!(send(&site, "POST", &cookie, ORIGIN, &submit)
        .await.status(), StatusCode::CONFLICT, "stale quote must not settle");
    assert_eq!(
        pg(&url, &format!(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"
        )),
        "700",
    );
    pg(&url, &format!("UPDATE chapters SET price_amount=450 WHERE id={target_ch}"));
    let finished = send(&site, "POST", &cookie, ORIGIN, &submit).await;
    assert_eq!(finished.status(), StatusCode::OK);
    assert!(html(finished).await.contains("Глава обменена"));
    let repeated = send(&site, "POST", &cookie, ORIGIN, &submit).await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert!(html(repeated).await.contains("Повторный запрос не списал деньги"));
    assert_eq!(pg(&url, &format!(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"
    )), "550");
    assert_eq!(pg(&url, &format!(
        "SELECT COUNT(*) FROM finance_direct_swap_postings
         WHERE source_purchase_id={source_purchase}"
    )), "1");
    assert_eq!(pg(&url, &format!(
        "SELECT COUNT(*) FROM chapter_purchase_revocations
         WHERE purchase_id={source_purchase} AND reason='purchase_swap'"
    )), "1");
    assert_eq!(pg(&url, &format!(
        "SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id={buyer}"
    )), "2");
    pool.close().await;
}
