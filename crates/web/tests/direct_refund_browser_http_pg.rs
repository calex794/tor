//! S036: full administrator support refund, no buyer self-service.
//! Integration tests run only on disposable PostgreSQL tor_ci.
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

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    form: &str,
) -> Response {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn html(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn hidden(markup: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    markup
        .split_once(&marker)
        .expect("expected hidden form field")
        .1
        .split_once('"')
        .expect("quoted field")
        .0
        .to_owned()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn s036_refunds_not_routed_to_buyer() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    for method in ["GET", "POST"] {
        assert_eq!(
            request(&site, method, "/me/refunds", "", ORIGIN, "purchase_id=1")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PostgreSQL, explicit support refund administrator opt-in"]
async fn s036_admin_support_search_exact_purchase_prepare_approve_and_replay() {
    assert_eq!(
        std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("TOR_ADMIN_SUPPORT_REFUNDS_ENABLED").as_deref(),
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
    let admin_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");
    assert_ne!(buyer, outsider);
    for method in ["GET", "POST"] {
        assert_eq!(
            request(
                &site,
                method,
                "/me/refunds",
                &buyer_cookie,
                ORIGIN,
                "purchase_id=1"
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    let chat: i64 = pg(
        &url,
        &format!(
            "INSERT INTO support_conversations(requester_user_id)
             VALUES({buyer}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO support_messages
             (conversation_id,sender_kind,sender_scope,sender_user_id,body)
             VALUES({chat},'user','user:{buyer}',{buyer},'Прошу рассмотреть возврат')"
        ),
    );
    let detail_path = format!("/admin/finance/refunds/user/{buyer}");
    assert_eq!(
        request(&site, "GET", &detail_path, &buyer_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&site, "GET", &detail_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({outsider},'administrator',true)
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=true"
        ),
    );
    let search = request(
        &site,
        "GET",
        "/admin/finance/refunds?q=Refund",
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(search.headers()[header::CACHE_CONTROL], "no-store");
    assert!(html(search).await.contains(&format!("user/{buyer}")));
    let detail = request(&site, "GET", &detail_path, &admin_cookie, ORIGIN, "").await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()["x-robots-tag"], "noindex, nofollow");
    let markup = html(detail).await;
    assert!(markup.contains(&format!("Покупка №{purchase}")));
    assert!(markup.contains("&lt;script&gt;"));
    assert!(!markup.contains("<script>x</script>"));
    assert!(markup.contains(&format!("/admin/support/{chat}")));
    assert!(markup.contains("action=\"/admin/finance/refunds/prepare\""));
    let command_key = hidden(&markup, "command_key");
    let body = format!(
        "buyer_id={buyer}&purchase_id={purchase}&conversation_id={chat}&command_key={command_key}&csrf_token={csrf}&reason=Support+goodwill"
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &buyer_cookie,
            ORIGIN,
            &body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &admin_cookie,
            "https://evil.invalid",
            &body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &admin_cookie,
            ORIGIN,
            &body.replace(&csrf, "invalid")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &admin_cookie,
            ORIGIN,
            &body.replace(
                &format!("conversation_id={chat}"),
                "conversation_id=9999999999"
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &admin_cookie,
            ORIGIN,
            &body
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/prepare",
            &admin_cookie,
            ORIGIN,
            &body
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM finance_support_refund_intents WHERE purchase_id={purchase}"
            )
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        "700"
    );
    let request_id = pg(
        &url,
        &format!(
            "SELECT request_id FROM finance_support_refund_intents WHERE purchase_id={purchase}"
        ),
    );
    let ready = html(request(&site, "GET", &detail_path, &admin_cookie, ORIGIN, "").await).await;
    assert!(ready.contains("action=\"/admin/finance/refunds/approve\""));
    let approval = format!(
        "csrf_token={csrf}&request_id={request_id}&expected_revision=1&reason=Approved+support+goodwill&current_password=S036RefundOtherPassword2026"
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/approve",
            &admin_cookie,
            "https://evil.invalid",
            &approval
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/refunds/approve",
            &admin_cookie,
            ORIGIN,
            &approval.replace("S036RefundOtherPassword2026", "BadPassword")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let accepted = request(
        &site,
        "POST",
        "/admin/finance/refunds/approve",
        &admin_cookie,
        ORIGIN,
        &approval,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(html(accepted).await.contains("Внутренний возврат"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        "1000"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id={purchase}"
            )
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM finance_direct_refund_postings WHERE purchase_id={purchase}"
            )
        ),
        "1"
    );
    let replay = request(
        &site,
        "POST",
        "/admin/finance/refunds/approve",
        &admin_cookie,
        ORIGIN,
        &approval,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(html(replay).await.contains("Повторное подтверждение"));
    assert_eq!(
        pg(&url,&format!("SELECT count(*) FROM reader_wallet_entries WHERE purchase_id={purchase} AND kind='chapter_refund'")),
        "1"
    );
    assert_eq!(
        pg(&url,&format!("SELECT count(*) FROM admin_action_audit WHERE action='finance.refund.approve' AND resource_id={purchase}")),
        "1"
    );
    pg(&url,&format!("UPDATE account_roles SET is_active=false WHERE user_id={outsider} AND role='administrator'"));
    assert_eq!(
        request(&site, "GET", &detail_path, &admin_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}
