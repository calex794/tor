//! S034: actual PostgreSQL purchase -> immutable private MinIO ciphertext ->
//! authenticated A/D/B/G HTTP grant; Node WebCrypto opens the signed GET.
//! Only disposable tor_ci, local MinIO and ephemeral signed credentials.
#![cfg(feature = "s3-paid")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    env, fs::OpenOptions, io::Write, num::NonZeroU32, path::Path, process::Command, sync::Arc,
    time::Duration,
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::{
    paid_storage::{PaidPublication, PgPaidStorage},
    PgCatalog,
};
use tor_commerce::{
    domain::money::{CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    repositories::{
        checkout::PgCheckout,
        entitlements::{MixedTocAccess, PgDirectEntitlements},
    },
};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{
        paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry, s3::S3PrivateStore,
    },
    services::{
        crypto::{seal_chapter, ContentKek},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};
use tower::ServiceExt;

#[path = "paid_grants_s3_pg/layer_fixtures.rs"]
mod layer_fixtures;
#[path = "paid_grants_s3_pg/toc_fixtures.rs"]
mod toc_fixtures;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const BUCKET: &str = "tor-private-chapter-ci";
const ORIGIN: &str = "http://127.0.0.1:3098";
const OBJECT_ID: &str = "f0c0e034-2026-4034-8034-000000000034";
const OBJECT_ID_SECOND: &str = "f0c0e034-2026-4034-8034-000000000035";
const CHAPTER_TWO_TITLE: &str = "Вторая купленная глава <script>опасно</script>";
const MARKDOWN: &str = "Оплаченная V1: кириллица А😀Б\n\n第二章 — только купленная редакция";
const BOOK_TITLE: &str = "Невбу <img src=x onerror=window.__xss=true> книга";
const CHAPTER_TITLE: &str = "Редакция «V1» — Секретная глава";

fn pg(url: &str, sql: &str) -> String {
    assert!(
        url.starts_with("postgresql://tor_app:")
            && url.ends_with("/tor_ci")
            && url.contains("@127.0.0.1:5432/"),
        "S034 tests require the disposable local tor_ci only"
    );
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
        .expect("PostgreSQL client is required");
    assert!(
        output.status.success(),
        "fixture PostgreSQL operation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn scalar(url: &str, sql: &str) -> i64 {
    pg(url, sql).parse().expect("one integer PG result")
}

async fn user(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let token = generate_session_token();
    let password = hash_password("S034RealStorageE2EPassword2026").unwrap();
    let id = accounts
        .register_with_session(
            email,
            "S034 private test",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, token)
}

fn grant_path(chapter: i64) -> String {
    format!("/api/v1/reader/chapters/{chapter}/delivery-grants")
}

async fn post_grant(
    app: &Router,
    chapter: i64,
    session: Option<&str>,
    csrf: Option<&str>,
    origin: &str,
    body: &str,
) -> Response {
    let mut request = Request::builder()
        .uri(grant_path(chapter))
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, origin);
    if let Some(session) = session {
        request = request.header(
            header::COOKIE,
            format!(
                "tor-dev-session={session}; tor-dev-csrf={}",
                csrf.unwrap_or("A")
            ),
        );
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn request_completion(
    app: &Router,
    method: &str,
    edition: (i64, i64, i64),
    session: &str,
    csrf: &str,
    body: Option<Value>,
) -> Response {
    let (chapter, purchase, version) = edition;
    let url = format!(
        "/api/v1/me/progress/purchased/complete?chapter_id={chapter}&purchase_id={purchase}&version_id={version}"
    );
    let mut request = Request::builder()
        .uri(if method == "GET" {
            url.as_str()
        } else {
            "/api/v1/me/progress/purchased/complete"
        })
        .method(method)
        .header(header::ORIGIN, ORIGIN)
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        )
        .header("x-csrf-token", csrf);
    if method == "POST" {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.map_or_else(String::new, |v| v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn decode(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap()
}

async fn paid_page(app: &Router, chapter: i64, purchase: i64, session: &str) -> Response {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/read/paid/{chapter}?purchase_id={purchase}"))
        .header(header::COOKIE, format!("tor-dev-session={session}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn private_get(app: &Router, uri: &str, session: Option<&str>) -> Response {
    let mut request = Request::builder().method("GET").uri(uri);
    if let Some(session) = session {
        request = request.header(header::COOKIE, format!("tor-dev-session={session}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn html_body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn save_ephemeral_fixture(
    grant: Value,
    version: i64,
    purchase: i64,
    readers: (String, String),
    reader_csp: String,
    expiry_probe_url: Value,
    toc_target: (i64, i64, i64, i64, i64),
) {
    let (reader_html, layer_reader_html) = readers;
    let target = env::var("TOR_PAID_GRANT_FIXTURE_PATH").expect("ephemeral fixture output");
    let runner_temp = env::var("RUNNER_TEMP").expect("CI temp directory");
    assert!(
        Path::new(&target).starts_with(&runner_temp),
        "never store scoped secrets inside the source checkout"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&target).expect("private fixture output");
    let payload = json!({
        "grant": grant,
        "purchased_version_id": version.to_string(),
        "purchase_id": purchase.to_string(),
        "reader_html": reader_html,
        "layer_reader_html": layer_reader_html,
        "reader_csp": reader_csp,
        "expiry_probe_url": expiry_probe_url,
        "expected_markdown": MARKDOWN,
        "expected_book_title": BOOK_TITLE,
        "expected_chapter_title": CHAPTER_TITLE,
        "expected_chapter_no": 1,
        "expected_version_no": 1,
        "toc_second_chapter_id": toc_target.0.to_string(),
        "toc_second_purchase_id": toc_target.1.to_string(),
        "toc_second_title": CHAPTER_TWO_TITLE,
        "toc_free_chapter_id": toc_target.2.to_string(),
        "toc_locked_chapter_id": toc_target.3.to_string(),
        "toc_draft_chapter_id": toc_target.4.to_string(),
    });
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18, local MinIO and NODE WebCrypto follow-up"]
async fn s034_paid_minio_actual_signed_get_from_postgres_purchase() {
    let url = env::var("TEST_DATABASE_URL").expect("PG disposable URL");
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    let key = env::var("TOR_TEST_S3_ACCESS_KEY").unwrap();
    let secret = env::var("TOR_TEST_S3_SECRET_KEY").unwrap();
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(key, secret, None, None, "local-test"))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("only a new isolated MinIO bucket");
    let s3 = S3PrivateStore::new(client, BUCKET.to_owned()).unwrap();

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let (author, _author_cookie) = user(&accounts, "s034-minio-author@tor-ci.example").await;
    let (buyer, buyer_cookie) = user(&accounts, "s034-minio-buyer@tor-ci.example").await;
    let (stranger, stranger_cookie) = user(&accounts, "s034-minio-stranger@tor-ci.example").await;
    let csrf = generate_session_token();

    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) \
             VALUES({buyer},600,'RUB')"
        ),
    );
    let book_title_sql = BOOK_TITLE.replace('\'', "''");
    let chapter_title_sql = CHAPTER_TITLE.replace('\'', "''");
    let book = scalar(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) \
             VALUES('ci:s034-private-e2e','ru','{book_title_sql}','public',{author}) \
             RETURNING id"
        ),
    );
    let chapter = scalar(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},1,'{chapter_title_sql}','public','paid',175) RETURNING id"
        ),
    );
    let sql_body = MARKDOWN.replace('\'', "''");
    let v1 = scalar(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter},1,'{sql_body}') RETURNING id"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({chapter},{v1})"
        ),
    );
    let pinned = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(v1).unwrap(),
    };
    let object_id = ObjectId::parse(OBJECT_ID).unwrap();
    let kek = Arc::new(ContentKek::from_bytes([0x63; 32]));
    let sealed = seal_chapter(pinned, object_id.clone(), MARKDOWN, &kek).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let reference = registry
        .reserve_pending(&sealed, &kek, BUCKET)
        .await
        .unwrap();
    let receipt = upload_and_verify(&s3, &reference, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    PgPaidStorage::new(pool.clone())
        .attach_verified_and_publish(
            author,
            book,
            pinned,
            &object_id,
            PaidPublication::KeepCurrent,
        )
        .await
        .unwrap();

    let short_store = s3.clone();
    let signer = PgPrivateDelivery::new(pool.clone(), s3, kek.clone(), BUCKET.to_owned()).unwrap();
    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
        (Arc::new(signer), ENDPOINT.to_owned()),
        None,
    )
    .unwrap();
    assert_eq!(
        post_grant(
            &app,
            chapter,
            Some(&buyer_cookie),
            Some(&csrf),
            ORIGIN,
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "buyer may not receive bytes before the payment commits"
    );
    assert_eq!(
        private_get(&app, "/me/purchases", None).await.status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        private_get(&app, "/me/purchases?after=-2", Some(&buyer_cookie))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    let empty = private_get(&app, "/me/purchases", Some(&buyer_cookie)).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(!html_body(empty).await.contains("Читать купленную редакцию"));

    assert_eq!(
        post_grant(&app, chapter, None, None, ORIGIN, "{}")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let layer_reader_html = layer_fixtures::assert_layer_s3_grants(
        tor_commerce::repositories::layers::PgLayers::new(pool.clone()),
        &app,
        &url,
        (author, stranger),
        (book, chapter, v1),
        (&stranger_cookie, &csrf),
    )
    .await;
    let purchase = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                PositiveId::new(buyer).unwrap(),
                WalletId::new(buyer).unwrap(),
                pinned,
                MoneyMinor::new(175).unwrap(),
                IdempotencyKey::parse(&format!("{}4", "Z".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let purchase_id = purchase.purchase_id.get();
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "checkout itself cannot record first protected issuance"
    );
    // A second fully verified paid edition proves that the TOC contains
    // live links, not synthetic chapter IDs or a public TOC.
    let chapter_two = scalar(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},2,'Вторая купленная глава <script>опасно</script>','public','paid',175) RETURNING id"
        ),
    );
    let version_two = scalar(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter_two},1,'Second paid edition content') RETURNING id"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({chapter_two},{version_two})"
        ),
    );
    let pinned_two = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter_two).unwrap(),
        version_id: PositiveId::new(version_two).unwrap(),
    };
    let object_two = ObjectId::parse(OBJECT_ID_SECOND).unwrap();
    let sealed_two = seal_chapter(
        pinned_two,
        object_two.clone(),
        "Second paid edition content",
        &kek,
    )
    .unwrap();
    let reference_two = registry
        .reserve_pending(&sealed_two, &kek, BUCKET)
        .await
        .unwrap();
    let receipt_two = upload_and_verify(&short_store, &reference_two, &sealed_two.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt_two).await.unwrap();
    PgPaidStorage::new(pool.clone())
        .attach_verified_and_publish(
            author,
            book,
            pinned_two,
            &object_two,
            PaidPublication::KeepCurrent,
        )
        .await
        .unwrap();
    let purchase_two = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                PositiveId::new(buyer).unwrap(),
                WalletId::new(buyer).unwrap(),
                pinned_two,
                MoneyMinor::new(175).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "Y".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let purchase_two_id = purchase_two.purchase_id.get();
    let second_reader = paid_page(&app, chapter_two, purchase_two_id, &buyer_cookie).await;
    assert_eq!(second_reader.status(), StatusCode::OK);
    let second_html = html_body(second_reader).await;
    assert!(second_html.contains("data-paid-toc"));
    assert!(second_html.contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")));
    assert!(!second_html.contains("<script>опасно</script>"));

    let stranger_list = private_get(&app, "/me/purchases", Some(&stranger_cookie)).await;
    assert_eq!(stranger_list.status(), StatusCode::OK);
    assert!(
        !html_body(stranger_list)
            .await
            .contains(&format!("purchase_id={purchase_id}")),
        "another account cannot enumerate purchases"
    );
    let owned_list = private_get(&app, "/me/purchases", Some(&buyer_cookie)).await;
    assert_eq!(owned_list.status(), StatusCode::OK);
    assert_eq!(owned_list.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(owned_list.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(owned_list.headers()["x-robots-tag"], "noindex");
    assert!(html_body(owned_list)
        .await
        .contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")));
    let ordinary_library = private_get(&app, "/me/library", Some(&buyer_cookie)).await;
    assert_eq!(ordinary_library.status(), StatusCode::OK);
    assert!(html_body(ordinary_library)
        .await
        .contains("href=\"/me/purchases\""));
    assert_eq!(
        post_grant(
            &app,
            chapter,
            Some(&stranger_cookie),
            Some(&csrf),
            ORIGIN,
            &json!({"purchase_id":purchase_id.to_string()}).to_string(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "another actor cannot reuse the buyer's purchase ID"
    );
    assert_eq!(
        post_grant(
            &app,
            chapter,
            Some(&buyer_cookie),
            Some(&csrf),
            "http://evil.example",
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let purchased = post_grant(
        &app,
        chapter,
        Some(&buyer_cookie),
        Some(&csrf),
        ORIGIN,
        "{}",
    )
    .await;
    assert_eq!(purchased.status(), StatusCode::OK);
    assert_eq!(
        purchased.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(purchased.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(purchased.headers()["x-robots-tag"], "noindex");
    let first = decode(purchased).await;
    assert_eq!(first["version_id"], v1.to_string());
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT chapter_id || ':' || version_id || ':' || object_kind
                 FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        format!("{chapter}:{v1}:chapter"),
        "real paid MinIO response must mark the exact direct purchased V1"
    );
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_two_id}"
            ),
        ),
        0,
        "opening one purchased chapter cannot mark a different purchase"
    );
    assert_eq!(first["schema"], "tor.paid_delivery.v1");
    assert!(!first.to_string().contains(MARKDOWN), "no plaintext in API");

    let (free_chapter, locked_chapter, draft_chapter) =
        toc_fixtures::create_mixed_toc_fixtures(&url, book);
    assert_eq!(
        private_get(&app, &format!("/reader/{free_chapter}"), None)
            .await
            .status(),
        StatusCode::OK,
    );
    let paywall = private_get(&app, &format!("/reader/{locked_chapter}"), None).await;
    assert_eq!(paywall.status(), StatusCode::OK);
    assert_eq!(paywall.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(paywall.headers()["x-robots-tag"], "noindex, nofollow");
    let paywall_html = String::from_utf8(
        axum::body::to_bytes(paywall.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(paywall_html.contains("Для чтения нужен доступ"));
    assert!(!paywall_html.contains(MARKDOWN));

    let v2 = scalar(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter},2,'New V2, not purchased') RETURNING id"
        ),
    );
    pg(
        &url,
        &format!("UPDATE chapter_publications SET version_id={v2} WHERE chapter_id={chapter}"),
    );
    // Capture the actual Rust HTML before the author hides the book. CI
    // screenshots exercise the mixed original NEwBOO-style free/paid/locked
    // design while preserving a separate hidden-book entitlement check.
    let visible_page = paid_page(&app, chapter, purchase_id, &buyer_cookie).await;
    assert_eq!(visible_page.status(), StatusCode::OK);
    let visible_html = html_body(visible_page).await;
    assert!(visible_html.contains("data-toc-state=\"free\""));
    assert_eq!(visible_html.matches("data-paid-chapter-nav").count(), 2);
    assert!(visible_html.contains("data-paid-nav-prev aria-disabled=\"true\""));
    assert!(visible_html.contains(&format!(
        "data-paid-nav-next rel=\"next\" href=\"/read/paid/{chapter_two}?purchase_id={purchase_two_id}\""
    )));
    assert!(visible_html.contains("data-toc-state=\"owned\""));
    assert!(visible_html.contains("data-toc-state=\"locked\""));
    assert!(visible_html.contains(&format!("href=\"/reader/{free_chapter}\"")));
    assert!(visible_html.contains(&format!(
        "href=\"/read/paid/{chapter_two}?purchase_id={purchase_two_id}\""
    )));
    assert!(visible_html.contains("1,75 ₽"));
    assert!(!visible_html.contains(&format!("href=\"/reader/{locked_chapter}\"")));
    assert!(!visible_html.contains("Unpurchased body never in TOC"));
    assert!(!visible_html.contains("SECRET_DRAFT_CHAPTER"));
    assert!(!visible_html.contains("SECRET_FUTURE_CHAPTER"));
    assert!(!visible_html.contains("<svg onload="));
    assert!(!visible_html.contains("<img src=x"));

    // Chapter 2 has both a verified paid predecessor (V1) and a publicly
    // readable free successor. Neither link is derived from progress data.
    let adjacent = paid_page(&app, chapter_two, purchase_two_id, &buyer_cookie).await;
    assert_eq!(adjacent.status(), StatusCode::OK);
    let adjacent_html = html_body(adjacent).await;
    assert!(adjacent_html.contains(&format!(
        "data-paid-nav-prev rel=\"prev\" href=\"/read/paid/{chapter}?purchase_id={purchase_id}\""
    )));
    assert!(adjacent_html.contains(&format!(
        "data-paid-nav-next rel=\"next\" href=\"/reader/{free_chapter}\""
    )));
    assert!(!adjacent_html.contains(&format!("href=\"/reader/{locked_chapter}\"")));
    assert!(!adjacent_html.contains("SECRET_DRAFT_CHAPTER"));

    pg(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={book}"),
    );
    pg(
        &url,
        &format!("UPDATE chapters SET visibility='draft' WHERE id={chapter}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility FROM books WHERE id={book}")
        ),
        "draft",
        "author-hide must persist before private TOC is rendered"
    );
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT count(*) FROM chapters c JOIN books b ON b.id=c.book_id
                 WHERE c.book_id={book} AND b.visibility='public'
                   AND c.visibility='public'"
            ),
        ),
        0,
        "B must not enumerate the author's hidden public chapter metadata"
    );
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT count(*) FROM library_purchased_chapters
                 WHERE user_id={buyer} AND chapter_id={free_chapter}"
            ),
        ),
        0,
        "public free chapter must not accidentally become a paid entitlement"
    );
    let scoped_toc = PgDirectEntitlements::new(pool.clone())
        .mixed_book_toc(
            PositiveId::new(buyer).unwrap(),
            PositiveId::new(chapter).unwrap(),
            PositiveId::new(purchase_id).unwrap(),
        )
        .await
        .unwrap();
    assert!(
        scoped_toc
            .chapters
            .iter()
            .all(|row| matches!(row.access, MixedTocAccess::Purchased { .. })),
        "hidden public metadata leaked from SQL query: {:?}",
        scoped_toc
            .chapters
            .iter()
            .map(|row| (row.chapter_id, &row.access))
            .collect::<Vec<_>>()
    );
    // When the author hides the book, the real HTTP menu must match the
    // already-checked D query: own pinned editions only, no public metadata.
    let hidden_page = paid_page(&app, chapter, purchase_id, &buyer_cookie).await;
    assert_eq!(hidden_page.status(), StatusCode::OK);
    let hidden_html = html_body(hidden_page).await;
    assert!(hidden_html.contains("data-toc-state=\"current\""));
    assert!(!hidden_html.contains("data-toc-state=\"free\""));
    assert!(!hidden_html.contains("data-toc-state=\"locked\""));
    let preserved = post_grant(
        &app,
        chapter,
        Some(&buyer_cookie),
        Some(&csrf),
        ORIGIN,
        "{}",
    )
    .await;
    assert_eq!(preserved.status(), StatusCode::OK);
    let historical_list = private_get(&app, "/me/purchases", Some(&buyer_cookie)).await;
    assert_eq!(historical_list.status(), StatusCode::OK);
    assert!(html_body(historical_list)
        .await
        .contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")));
    let still_v1 = decode(preserved).await;
    assert_eq!(still_v1["version_id"], v1.to_string());
    assert!(!still_v1.to_string().contains(MARKDOWN));
    // A D purchase stays readable even while an author hides the work,
    // but F's visible private ranking deliberately excludes unpublished
    // books. Re-publish after exercising D's hidden V1 rights so the next
    // assertion tests F eligibility instead of violating its visibility rule.
    pg(
        &url,
        &format!(
            "UPDATE books SET visibility='public' WHERE id={book}; \
             UPDATE chapters SET visibility='public' WHERE id={chapter}"
        ),
    );

    // C/F-05: A grant, downloaded ciphertext or bookmark alone does not
    // create a ranking fact. The owner explicitly finishes the sealed V1.
    let before_finish = request_completion(
        &app,
        "GET",
        (chapter, purchase_id, v1),
        &buyer_cookie,
        &csrf,
        None,
    )
    .await;
    assert_eq!(before_finish.status(), StatusCode::OK);
    let state = decode(before_finish).await;
    assert_eq!(state["available"], true);
    assert_eq!(state["completed"], false);
    let sha = hex::encode(sealed.plaintext_sha256);
    let final_block = MARKDOWN.split("\n\n").last().unwrap();
    let command = json!({
        "purchase_id": purchase_id.to_string(),
        "chapter_id": chapter.to_string(),
        "version_id": v1.to_string(),
        "block_id": 2,
        "char_offset": final_block.encode_utf16().count(),
        "expected_revision": 0,
        "plaintext_sha256": sha,
    });
    let denied = request_completion(
        &app,
        "POST",
        (chapter, purchase_id, v1),
        &stranger_cookie,
        &csrf,
        Some(command.clone()),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let mut fake = command.clone();
    fake["char_offset"] = json!(1);
    let fake_response = request_completion(
        &app,
        "POST",
        (chapter, purchase_id, v1),
        &buyer_cookie,
        &csrf,
        Some(fake),
    )
    .await;
    assert_eq!(fake_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let rank_before = private_get(&app, "/api/v1/me/rankings", Some(&buyer_cookie)).await;
    assert!(!decode(rank_before)
        .await
        .to_string()
        .contains(&book.to_string()));
    let done = request_completion(
        &app,
        "POST",
        (chapter, purchase_id, v1),
        &buyer_cookie,
        &csrf,
        Some(command.clone()),
    )
    .await;
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(decode(done).await["status"], "completed");
    let duplicate = request_completion(
        &app,
        "POST",
        (chapter, purchase_id, v1),
        &buyer_cookie,
        &csrf,
        Some(command.clone()),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(decode(duplicate).await["status"], "already_completed");
    let after = decode(private_get(&app, "/api/v1/me/rankings", Some(&buyer_cookie)).await).await;
    assert!(after.to_string().contains(&book.to_string()));
    assert!(
        !after.to_string().contains(MARKDOWN),
        "ranking may not reveal paid body"
    );
    let completed_state = decode(
        request_completion(
            &app,
            "GET",
            (chapter, purchase_id, v1),
            &buyer_cookie,
            &csrf,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(completed_state["completed"], true);

    // Save a real server-rendered page, not a hand-written copy of its HTML.
    // Buyer owns V1 after V2/creator-hide, but a stranger cannot open the page.
    let stranger = paid_page(&app, chapter, purchase_id, &stranger_cookie).await;
    assert_eq!(stranger.status(), StatusCode::FORBIDDEN);
    let page = paid_page(&app, chapter, purchase_id, &buyer_cookie).await;
    assert_eq!(page.status(), StatusCode::OK);
    let reader_csp = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    let reader_html = String::from_utf8(
        to_bytes(page.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(reader_html.contains("data-paid-chapter-reader"));
    assert!(reader_html.contains("data-paid-toc"));
    assert!(reader_html.contains("Оглавление книги"));
    // Main republished the book before the F-05 ranking scenario below.
    // The reader should therefore show public metadata again.
    assert!(reader_html.contains("data-toc-state=\"free\""));
    assert!(reader_html.contains("data-toc-state=\"locked\""));
    assert!(reader_html.contains(&format!(
        "/read/paid/{chapter_two}?purchase_id={purchase_two_id}"
    )));
    assert!(reader_html.contains("Вторая купленная глава &lt;script&gt;опасно&lt;/script&gt;"));
    assert!(!reader_html.contains("<script>опасно</script>"));
    assert!(reader_html.contains("<body class=\"reader-standalone\">"));
    assert!(reader_html.contains("<main id=\"main\" class=\"reader-standalone__main\">"));
    assert!(!reader_html.contains("class=\"site-header\""));
    assert!(!reader_html.contains("class=\"site-footer\""));
    assert!(reader_html.contains("Невбу &lt;img src=x onerror=window.__xss=true&gt; книга"));
    assert!(reader_html.contains(CHAPTER_TITLE));
    assert!(reader_html.contains("Глава 1. Редакция «V1»"));
    assert!(reader_html.contains("Купленная редакция 1"));
    assert!(!reader_html.contains("<img src=x"));
    assert!(!reader_html.contains(MARKDOWN));
    assert!(!reader_html.contains("dek_b64u"));
    let expiry_probe_url = serde_json::to_value(
        short_store
            .presign_verified(&reference, Duration::from_secs(30))
            .await
            .unwrap(),
    )
    .unwrap();
    save_ephemeral_fixture(
        still_v1,
        v1,
        purchase_id,
        (visible_html, layer_reader_html),
        reader_csp,
        expiry_probe_url,
        (
            chapter_two,
            purchase_two_id,
            free_chapter,
            locked_chapter,
            draft_chapter,
        ),
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        paid_page(&app, chapter, purchase_id, &buyer_cookie)
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    let held_list = private_get(&app, "/me/purchases", Some(&buyer_cookie)).await;
    assert!(!html_body(held_list)
        .await
        .contains(&format!("purchase_id={purchase_id}")));
    assert_eq!(
        post_grant(
            &app,
            chapter,
            Some(&buyer_cookie),
            Some(&csrf),
            ORIGIN,
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) \
             VALUES({purchase_id},'ci:s034-minio-refund','refund')"
        ),
    );
    assert_eq!(
        paid_page(&app, chapter, purchase_id, &buyer_cookie)
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    let refunded_list = private_get(&app, "/me/purchases", Some(&buyer_cookie)).await;
    assert!(!html_body(refunded_list)
        .await
        .contains(&format!("purchase_id={purchase_id}")));
    assert_eq!(
        post_grant(
            &app,
            chapter,
            Some(&buyer_cookie),
            Some(&csrf),
            ORIGIN,
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let revoked_finish = request_completion(
        &app,
        "POST",
        (chapter, purchase_id, v1),
        &buyer_cookie,
        &csrf,
        Some(command),
    )
    .await;
    assert_eq!(revoked_finish.status(), StatusCode::NOT_FOUND);
    // Previous signed links remain bearer credentials until TTL; the Node
    // follow-up consumes this just-issued URL and validates its real S3 GET.
    pool.close().await;
}
