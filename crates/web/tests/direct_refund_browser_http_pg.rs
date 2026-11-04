//! S036: buyer refund request browser against disposable PostgreSQL only.
//! No approval endpoint, payouts, production policy or retained database.
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
use tor_commerce::{
    domain::money::{CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PATH: &str = "/me/refunds";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use a retained database");
    let result = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=15000 -c lock_timeout=5000",
        )
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
        .expect("psql is required for disposable financial acceptance");
    assert!(
        result.status.success(),
        "refund fixture SQL failed: {}",
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

async fn admin_get(app: &Router, cookie: &str, path: &str) -> Response {
    let request = Request::builder()
        .uri(path)
        .method("GET")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn admin_post(app: &Router, cookie: &str, origin: &str, body: &str) -> Response {
    let request = Request::builder()
        .uri("/admin/finance/refunds/approve")
        .method("POST")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn admin_reject(app: &Router, cookie: &str, origin: &str, body: &str) -> Response {
    let request = Request::builder()
        .uri("/admin/finance/refunds/reject")
        .method("POST")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn html(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn hidden(markup: &str, name: &str) -> String {
    let prefix = format!("name=\"{name}\" value=\"");
    markup
        .split_once(&prefix)
        .expect("server must issue a buyer-bound form field")
        .1
        .split_once('"')
        .expect("form field must be quoted")
        .0
        .to_owned()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with refund feature disabled"]
async fn s036_refund_browser_disabled_by_default() {
    assert_ne!(
        std::env::var("TOR_DIRECT_REFUND_REQUESTS_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    assert_eq!(
        send(&site, "GET", "", ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&site, "POST", "", ORIGIN, "purchase_id=1")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(
        std::env::var("TOR_REFUND_APPROVAL_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    assert_eq!(
        admin_post(&site, "", ORIGIN, "invalid=form").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(
        std::env::var("TOR_REFUND_REJECTION_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    assert_eq!(
        admin_reject(&site, "", ORIGIN, "invalid=form")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci and explicit refund opt-in"]
async fn s036_buyer_http_owner_csrf_policy_submit_replay_and_history() {
    assert_eq!(
        std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("TOR_DIRECT_REFUND_REQUESTS_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let buyer = accounts
        .register_with_session(
            &format!("s036-buyer-{nonce}@example.invalid"),
            "Refund buyer",
            &hash_password("S036RefundBuyerPassword2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let outsider = accounts
        .register_with_session(
            &format!("s036-other-{nonce}@example.invalid"),
            "Other buyer",
            &hash_password("S036RefundOtherPassword2026").unwrap(),
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES({buyer},1000,'RUB')"),
    );
    let owner: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('s036-author-{nonce}@example.invalid','Author','$argon2id$ci') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:s036:book:{nonce}','ru','Книга <script>x</script>','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Глава <img>','public','paid',300) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Paid body') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    // This policy is a test-only fixture. It is never installed for actual users.
    let legal = format!("ci:s036:legal:{nonce}");
    let terms = format!("ci:s036:terms:{nonce}");
    pg(
        &url,
        &format!(
            "INSERT INTO finance_direct_refund_policy_catalog
             (version,approval_reference,approved_by_user_id,approved_at,effective_at,window_seconds)
             VALUES('{legal}','ci:s036:approval:{nonce}',{owner},
                    now()-INTERVAL '2 days',now()-INTERVAL '1 day',86400)"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO finance_revenue_terms
             (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
             VALUES({book},'{terms}',2000,'{legal}',86400)"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO finance_revenue_term_states(book_id,policy_version)
             VALUES({book},'{terms}')"
        ),
    );
    let key = IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap();
    let command = PurchaseCommand::new(
        PositiveId::new(buyer).unwrap(),
        WalletId::new(buyer).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        MoneyMinor::new(300).unwrap(),
        key,
    )
    .unwrap();
    let purchase = PgCheckout::new(pool)
        .with_creator_posting(true)
        .checkout_once(&command)
        .await
        .unwrap()
        .purchase_id
        .get();
    let site = app(accounts, catalog);
    let buyer_cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");
    assert_ne!(buyer, outsider);
    assert_eq!(
        send(&site, "GET", "", ORIGIN, "").await.status(),
        StatusCode::SEE_OTHER
    );
    let response = send(&site, "GET", &buyer_cookie, ORIGIN, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    let listing = html(response).await;
    assert!(listing.contains(&format!("name=\"purchase_id\" value=\"{purchase}\"")));
    assert!(listing.contains("&lt;script&gt;"));
    assert!(!listing.contains("<script>x</script>"));
    let foreign = html(send(&site, "GET", &outsider_cookie, ORIGIN, "").await).await;
    assert!(!foreign.contains(&format!("value=\"{purchase}\"")));
    let post = format!(
        "csrf_token={csrf}&purchase_id={purchase}&command_key={}&reason=Buyer+request",
        hidden(&listing, "command_key")
    );
    assert_eq!(
        send(&site, "POST", "", ORIGIN, &post).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&site, "POST", &buyer_cookie, "https://evil.invalid", &post)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &site,
            "POST",
            &buyer_cookie,
            ORIGIN,
            &post.replace(&csrf, "invalid")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &site,
            "POST",
            &buyer_cookie,
            ORIGIN,
            &format!("{post}&user_id={outsider}")
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(&site, "POST", &outsider_cookie, ORIGIN, &post)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let created = send(&site, "POST", &buyer_cookie, ORIGIN, &post).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert!(html(created).await.contains("Заявка создана"));
    let replay = send(&site, "POST", &buyer_cookie, ORIGIN, &post).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(html(replay).await.contains("Заявка уже существовала"));
    assert_eq!(
        send(
            &site,
            "POST",
            &buyer_cookie,
            ORIGIN,
            &post.replace("Buyer+request", "Changed+reason")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let after = html(send(&site, "GET", &buyer_cookie, ORIGIN, "").await).await;
    assert!(after.contains("На рассмотрении"));
    assert!(!after.contains(&format!("name=\"purchase_id\" value=\"{purchase}\"")));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM finance_direct_refund_requests WHERE purchase_id={purchase} AND state='pending'"
            ),
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"),
        ),
        "700"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id={purchase}"
            ),
        ),
        "0"
    );

    // Read-only S036 moderator queue: no PII or request reason for a
    // non-admin session, even when the purchase belongs to the caller.
    let admin_path = "/admin/finance/refunds";
    assert_eq!(
        admin_get(&site, "", admin_path).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        admin_get(&site, &buyer_cookie, admin_path).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        admin_get(&site, &outsider_cookie, admin_path)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM finance_direct_refund_admin_access_audit WHERE admin_user_id={outsider}"),
        ),
        "0"
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({outsider},'administrator',true)
             ON CONFLICT (user_id,role) DO UPDATE SET is_active=EXCLUDED.is_active"
        ),
    );
    let response = admin_get(&site, &outsider_cookie, admin_path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    let admin_html = html(response).await;
    let refund_id = pg(
        &url,
        &format!("SELECT id FROM finance_direct_refund_requests WHERE purchase_id={purchase}"),
    );
    assert!(admin_html.contains(&format!("Заявка №{refund_id}")));
    assert!(admin_html.contains("Buyer request"));
    assert!(admin_html.contains("&lt;script&gt;"));
    assert!(!admin_html.contains("<script>x</script>"));
    // The global site header has its own forms. The refund review section
    // must never provide a financial decision form or endpoint.
    assert!(!admin_html.contains("action=\"/admin/finance/refunds\""));
    assert!(!admin_html.contains("name=\"decision\""));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM finance_direct_refund_admin_access_audit WHERE admin_user_id={outsider}"),
        ),
        "1"
    );
    let approval_enabled = std::env::var("TOR_REFUND_APPROVAL_HTTP_ENABLED").as_deref() == Ok("1");
    let rejection_enabled =
        std::env::var("TOR_REFUND_REJECTION_HTTP_ENABLED").as_deref() == Ok("1");
    assert!(
        !(approval_enabled && rejection_enabled),
        "run dedicated acceptance fixtures for each exclusive decision"
    );
    if approval_enabled {
        assert!(admin_html.contains("action=\"/admin/finance/refunds/approve\""));
        let request_form = format!(
            "csrf_token={csrf}&request_id={refund_id}&expected_revision=1&reason=Admin+approved&current_password=S036RefundOtherPassword2026"
        );
        let wrong = request_form.replace("S036RefundOtherPassword2026", "IncorrectPassword");
        assert_eq!(
            admin_post(&site, &outsider_cookie, ORIGIN, &wrong)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            admin_post(
                &site,
                &outsider_cookie,
                "https://evil.invalid",
                &request_form
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let buyer_claim =
            request_form.replace("S036RefundOtherPassword2026", "S036RefundBuyerPassword2026");
        assert_eq!(
            admin_post(&site, &buyer_cookie, ORIGIN, &buyer_claim)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin_post(
                &site,
                &outsider_cookie,
                ORIGIN,
                &format!("{request_form}&user_id={buyer}")
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            pg(
                &url,
                &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
            ),
            "700"
        );
        let approved = admin_post(&site, &outsider_cookie, ORIGIN, &request_form).await;
        assert_eq!(approved.status(), StatusCode::OK);
        assert!(html(approved).await.contains("Внутренний возврат"));
        assert_eq!(
            pg(
                &url,
                &format!("SELECT state FROM finance_direct_refund_requests WHERE id={refund_id}")
            ),
            "approved"
        );
        assert_eq!(
            pg(
                &url,
                &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
            ),
            "1000"
        );
        assert_eq!(
            pg(&url, &format!("SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id={purchase}")),
            "1"
        );
        let replay = admin_post(&site, &outsider_cookie, ORIGIN, &request_form).await;
        assert_eq!(replay.status(), StatusCode::OK);
        assert!(html(replay).await.contains("Повторное подтверждение"));
        assert_eq!(
            pg(&url, &format!("SELECT count(*) FROM finance_direct_refund_postings WHERE purchase_id={purchase}")),
            "1"
        );
        assert_eq!(
            pg(
                &url,
                &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
            ),
            "1000"
        );
    } else {
        assert!(!admin_html.contains("action=\"/admin/finance/refunds/approve\""));
        assert_eq!(
            admin_post(&site, &outsider_cookie, ORIGIN, "invalid=form")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    if rejection_enabled {
        assert!(admin_html.contains("action=\"/admin/finance/refunds/reject\""));
        let reject_form = format!(
            "csrf_token={csrf}&request_id={refund_id}&expected_revision=1&reason=Admin+rejected&current_password=S036RefundOtherPassword2026"
        );
        let bad_password = reject_form.replace("S036RefundOtherPassword2026", "IncorrectPassword");
        assert_eq!(
            admin_reject(&site, &outsider_cookie, ORIGIN, &bad_password)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            admin_reject(
                &site,
                &outsider_cookie,
                "https://evil.invalid",
                &reject_form
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin_reject(
                &site,
                &outsider_cookie,
                ORIGIN,
                &reject_form.replace(&csrf, "invalid")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin_reject(&site, &buyer_cookie, ORIGIN, &reject_form)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin_reject(
                &site,
                &outsider_cookie,
                ORIGIN,
                &format!("{reject_form}&user_id={buyer}")
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            admin_reject(
                &site,
                &outsider_cookie,
                ORIGIN,
                &reject_form.replace("expected_revision=1", "expected_revision=2")
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        let rejected = admin_reject(&site, &outsider_cookie, ORIGIN, &reject_form).await;
        assert_eq!(rejected.status(), StatusCode::OK);
        assert!(html(rejected).await.contains("Заявка"));
        assert_eq!(
            pg(
                &url,
                &format!("SELECT state FROM finance_direct_refund_requests WHERE id={refund_id}")
            ),
            "rejected"
        );
        assert_eq!(
            pg(
                &url,
                &format!(
                    "SELECT revision FROM finance_direct_refund_requests WHERE id={refund_id}"
                )
            ),
            "2"
        );
        let replay = admin_reject(&site, &outsider_cookie, ORIGIN, &reject_form).await;
        assert_eq!(replay.status(), StatusCode::OK);
        assert!(html(replay).await.contains("Повтор решения"));
        assert_eq!(
            pg(
                &url,
                &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
            ),
            "700"
        );
        assert_eq!(
            pg(&url, &format!("SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id={purchase}")),
            "0"
        );
        assert_eq!(
            pg(&url, &format!("SELECT count(*) FROM finance_direct_refund_postings WHERE purchase_id={purchase}")),
            "0"
        );
    } else {
        assert!(!admin_html.contains("action=\"/admin/finance/refunds/reject\""));
        assert_eq!(
            admin_reject(&site, &outsider_cookie, ORIGIN, "invalid=form")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let older = html(
        admin_get(
            &site,
            &outsider_cookie,
            &format!("/admin/finance/refunds?before={refund_id}"),
        )
        .await,
    )
    .await;
    assert!(!older.contains(&format!("Заявка №{refund_id}")));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM finance_direct_refund_admin_access_audit WHERE admin_user_id={outsider}"),
        ),
        "2"
    );
    pg(
        &url,
        &format!("UPDATE account_roles SET is_active=false WHERE user_id={outsider} AND role='administrator'"),
    );
    assert_eq!(
        admin_get(&site, &outsider_cookie, admin_path)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM finance_direct_refund_admin_access_audit WHERE admin_user_id={outsider}"),
        ),
        "2"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"),
        ),
        if approval_enabled { "1000" } else { "700" }
    );
}
