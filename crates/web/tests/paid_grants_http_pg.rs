//! S034: real PostgreSQL/Axum grant boundary, without a real payment provider.
//! G is a deterministic signer fixture here; actual MinIO signing belongs to
//! tor-storage/s3_private_provider and the final full-stack staging suite.
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    num::NonZeroU32,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;
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
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, DeliveryError, EncryptedObjectRef, GrantedContent, ObjectId, PositiveId,
    PrivateObjectDeliveryPort, SecretValue,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const ENDPOINT: &str = "/api/v1/reader/chapters/";
const UUID: &str = "a1a1a1a1-1111-4111-8111-111111111111";
const BODY: &str = "S034-PAID-PRIVATE-V1-NEVER-IN-HTTP";

fn pg(url: &str, sql: &str) -> String {
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"),
        "PG test must only use disposable tor_ci as tor_app"
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
        .expect("PostgreSQL client required");
    assert!(
        output.status.success(),
        "isolated PG fixture error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn scalar(url: &str, sql: &str) -> i64 {
    pg(url, sql).parse().expect("one integer PostgreSQL result")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn account(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let token = generate_session_token();
    let hash = hash_password("S034VeryStrongTestPassword2026").unwrap();
    let id = accounts
        .register_with_session(
            email,
            "S034 test",
            &hash,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, token)
}

#[derive(Default)]
struct FakeG {
    signed: AtomicUsize,
    // Test-only barrier allows a refund to commit after G begins signing.
    pause_next: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

#[async_trait]
impl PrivateObjectDeliveryPort for FakeG {
    async fn grant_verified_object(
        &self,
        object: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        assert_eq!(ttl, Duration::from_secs(90));
        self.signed.fetch_add(1, Ordering::SeqCst);
        let barrier = { self.pause_next.lock().unwrap().take() };
        if let Some((entered, release)) = barrier {
            let _ = entered.send(());
            release.await.map_err(|_| DeliveryError::Unavailable)?;
        }
        Ok(GrantedContent {
            ciphertext_url: SecretValue::new(
                "https://private.invalid/V1.enc?X-Amz-Signature=TEST-ONLY".to_owned(),
            )
            .unwrap(),
            object_sha256: hex(&object.object.sha256_ciphertext),
            cipher: "AES-256-GCM",
            codec: "gzip",
            nonce_b64u: URL_SAFE_NO_PAD.encode(object.nonce),
            aad_b64u: URL_SAFE_NO_PAD.encode(&object.aad),
            dek_b64u: SecretValue::new(URL_SAFE_NO_PAD.encode([0x11_u8; 32])).unwrap(),
        })
    }
}

fn route(chapter: i64) -> String {
    format!("{ENDPOINT}{chapter}/delivery-grants")
}

async fn send(
    app: &Router,
    path: &str,
    session: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    json_body: &str,
) -> Response {
    let mut request = Request::builder()
        .uri(path)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        let csrf_cookie = csrf.unwrap_or("A");
        request = request.header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf_cookie}"),
        );
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    app.clone()
        .oneshot(request.body(Body::from(json_body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn read_page(app: &Router, path: &str, session: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path).method("GET");
    if let Some(session) = session {
        request = request.header(header::COOKIE, format!("tor-dev-session={session}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// S065 / C-12: the historical NEwBOO reader aliases must not become an
/// alternative entitlement. Only the D-checked paid grant may sign G objects.
/// Legacy endpoints are *not* implemented in TOR; a 404 proves absence on
/// the actual authenticated router, not parity with NEwBOO functionality.
async fn s065_alternate_reader_access_matrix(
    browser: &Router,
    pinned: (i64, i64, i64),
    buyer: &str,
    outsider: &str,
    outsider_csrf: &str,
) {
    let (chapter, version, purchase) = pinned;
    // Real public and account reader routes exist, but a forged preview or
    // purchased flag must not turn a paid chapter into public Markdown.
    for path in [
        format!("/reader/{chapter}?preview=1&version_id={version}&purchase_id={purchase}"),
        format!("/read/{chapter}?preview=1&version_id={version}&purchase_id={purchase}"),
        format!("/api/v1/chapters/{chapter}/read?preview=1&purchase_id={purchase}"),
    ] {
        for session in [buyer, outsider] {
            let response = read_page(browser, &path, Some(session)).await;
            let is_paywall = path.starts_with("/reader/");
            let expected = if is_paywall {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(response.status(), expected, "{path}");
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "no-store",
                "{path}"
            );
            if is_paywall {
                assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
            }
            let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
            assert!(
                !bytes
                    .windows(BODY.len())
                    .any(|part| part == BODY.as_bytes()),
                "paid plaintext leaked on {path}"
            );
            assert!(
                !bytes
                    .windows(b"X-Amz-Signature".len())
                    .any(|part| part == b"X-Amz-Signature"),
                "S3 signing data leaked on {path}"
            );
            if is_paywall {
                let html = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(html.contains("Для чтения нужен доступ"));
                assert!(html.contains("href=\"/login\""));
                assert!(
                    !html.contains(&format!("purchase_id={purchase}")),
                    "untrusted public query must not echo a purchase token"
                );
            } else {
                assert!(bytes.is_empty(), "paid compatibility route revealed data");
            }
        }
    }

    // NEwBOO navigation/history, preview and media are not TOR routes yet.
    // Keep them closed when the owner is logged in as well as for outsiders.
    for path in [
        format!("/reader/{chapter}/toc-fragment?limit=100"),
        format!("/reader/{chapter}/comments-fragment"),
        format!("/reader/{chapter}/history?version_id={version}"),
        format!("/reader/{chapter}/preview?version_id={version}"),
        format!("/api/v1/chapters/{chapter}/history"),
        format!("/api/v1/chapters/{chapter}/versions/{version}"),
        format!("/api/v1/chapters/{chapter}/preview"),
        format!("/cdn/chapters/{chapter}/{version}.html"),
        format!("/media/chapters/{chapter}/{version}"),
    ] {
        for session in [buyer, outsider] {
            let response = read_page(browser, &path, Some(session)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
            assert!(
                !bytes
                    .windows(BODY.len())
                    .any(|part| part == BODY.as_bytes()),
                "paid plaintext in disabled compatibility route {path}"
            );
            assert!(
                !bytes.windows(15).any(|part| part == b"X-Amz-Signature"),
                "signed S3 credential in disabled compatibility route {path}"
            );
        }
    }

    // Progress is data about a chapter, never an entitlement. In particular,
    // a stolen purchase ID cannot be used to read someone else's V1 position.
    let owned_query = format!(
        "/api/v1/me/progress/purchased?purchase_id={purchase}&chapter_id={chapter}&version_id={version}"
    );
    let owned = read_page(browser, &owned_query, Some(buyer)).await;
    assert_eq!(owned.status(), StatusCode::OK);
    assert_eq!(owned.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(owned.into_body(), 8192).await.unwrap();
    assert!(!bytes
        .windows(BODY.len())
        .any(|part| part == BODY.as_bytes()));

    for path in [
        owned_query,
        format!(
            "/api/v1/me/progress/purchased/complete?purchase_id={purchase}&chapter_id={chapter}&version_id={version}"
        ),
    ] {
        let denied = read_page(browser, &path, Some(outsider)).await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
        assert!(to_bytes(denied.into_body(), 8192).await.unwrap().is_empty());
    }
    let free = read_page(
        browser,
        &format!("/api/v1/me/progress/free?chapter_id={chapter}"),
        Some(buyer),
    )
    .await;
    assert_eq!(free.status(), StatusCode::NOT_FOUND);
    assert_eq!(free.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(free.into_body(), 8192).await.unwrap().is_empty());

    let forged = json!({
        "purchase_id": purchase.to_string(),
        "chapter_id": chapter.to_string(),
        "version_id": version.to_string(),
        "block_id": 1,
        "char_offset": 0,
        "expected_revision": 0,
    });
    let request = Request::builder()
        .uri("/api/v1/me/progress/purchased")
        .method("PUT")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header("x-csrf-token", outsider_csrf)
        .header(
            header::COOKIE,
            format!("tor-dev-session={outsider}; tor-dev-csrf={outsider_csrf}"),
        )
        .body(Body::from(forged.to_string()))
        .unwrap();
    let denied = browser.clone().oneshot(request).await.unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(denied.into_body(), 8192).await.unwrap().is_empty());
}

async fn json_body(reply: Response) -> Value {
    let bytes = to_bytes(reply.into_body(), 8192).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL TEST_DATABASE_URL ending /tor_ci"]
async fn s034_real_http_pinned_v1_sessions_csrf_refund_and_creator_hide() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI url");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let reader = PgReader::new(pool.clone());
    let (buyer, buyer_cookie) = account(&accounts, "s034-http-buyer@tor-ci.example").await;
    let (author, author_cookie) = account(&accounts, "s034-http-author@tor-ci.example").await;
    let csrf = generate_session_token();
    let author_csrf = generate_session_token();

    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) \
             VALUES({buyer},600,'RUB')"
        ),
    );
    let book = scalar(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) \
             VALUES('ci:s034-http-grant','ru','Платная глава','public',{author}) RETURNING id"
        ),
    );
    let chapter = scalar(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},1,'Paid V1','public','paid',175) RETURNING id"
        ),
    );
    let version = scalar(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter},1,'{BODY}') RETURNING id"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({chapter},{version})"
        ),
    );
    let pin = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    };
    let digest = hex(&Sha256::digest(BODY.as_bytes()));
    let aad = format!("tor.encrypted.chapter.v1\n{chapter}\n{version}\n{UUID}\n{digest}\n");
    let sha = hex(&[4_u8; 32]);
    pg(
        &url,
        &format!(
            "INSERT INTO storage_objects(\
                id,object_kind,bucket,object_key,state,size_bytes,\
                sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,\
                kek_id,object_nonce,object_aad,codec,verified_at,delete_after\
             ) VALUES (\
                '{UUID}'::uuid,'chapter','tor-private-chapter-ci',\
                'private/chapter-versions/{UUID}.enc','ready',64,\
                decode('{sha}','hex'),decode('{digest}','hex'),\
                decode(repeat('08',48),'hex'),decode(repeat('03',12),'hex'),\
                'content-kek-v1',decode(repeat('02',12),'hex'),\
                decode('{}','hex'),'gzip',now(),now()+INTERVAL '24 hours'\
             )",
            hex(aad.as_bytes())
        ),
    );
    PgPaidStorage::new(pool.clone())
        .attach_verified_and_publish(
            author,
            book,
            pin,
            &ObjectId::parse(UUID).unwrap(),
            PaidPublication::KeepCurrent,
        )
        .await
        .unwrap();
    let fake = Arc::new(FakeG::default());
    let browser = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        catalog.clone(),
        reader.clone(),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        (fake.clone(), "http://127.0.0.1:9000".to_owned()),
        None,
    )
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth_paid_grants(
        catalog,
        reader,
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        fake.clone(),
        None,
    );
    let path = route(chapter);
    let pre_purchase = send(
        &app,
        &path,
        Some(&buyer_cookie),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(pre_purchase.status(), StatusCode::FORBIDDEN);
    assert_eq!(fake.signed.load(Ordering::SeqCst), 0);
    assert_eq!(
        send(&app, &path, None, None, Some(ORIGIN), "{}")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            &path,
            Some(&buyer_cookie),
            Some(&csrf),
            Some("https://evil.example"),
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, &path, Some(&buyer_cookie), None, Some(ORIGIN), "{}")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            &path,
            Some(&author_cookie),
            Some(&author_csrf),
            Some(ORIGIN),
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fake.signed.load(Ordering::SeqCst), 0);

    assert_eq!(
        read_page(&browser, "/read/paid/1?purchase_id=1", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    // The API-only router never mounts the H reader.
    assert_eq!(
        read_page(&app, "/read/paid/1?purchase_id=1", Some(&buyer_cookie))
            .await
            .status(),
        StatusCode::NOT_FOUND,
    );

    let checkout = PgCheckout::new(pool.clone());
    let receipt = checkout
        .checkout_once(
            &PurchaseCommand::new(
                PositiveId::new(buyer).unwrap(),
                WalletId::new(buyer).unwrap(),
                pin,
                MoneyMinor::new(175).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let purchase_id = receipt.purchase_id.get();
    let reader_path = format!("/read/paid/{chapter}?purchase_id={purchase_id}");
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "checkout is not a protected read"
    );
    let stranger = read_page(&browser, &reader_path, Some(&author_cookie)).await;
    assert_eq!(stranger.status(), StatusCode::FORBIDDEN);
    let page = read_page(&browser, &reader_path, Some(&buyer_cookie)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(page.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(page.headers()["x-robots-tag"], "noindex");
    assert!(page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("connect-src 'self' http://127.0.0.1:9000"));
    let html = String::from_utf8(
        to_bytes(page.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("data-paid-chapter-reader"));
    assert!(html.contains("newboo-paid-reader.mjs"));
    assert!(html.contains(&format!("data-purchase-id=\"{purchase_id}\"")));
    assert!(
        !html.contains(BODY),
        "S034 raw plaintext must not enter SSR HTML"
    );
    assert!(!html.contains("dek_b64u"), "S034 no scoped DEK in SSR HTML");
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "reader shell is not first paid-grant evidence"
    );
    s065_alternate_reader_access_matrix(
        &browser,
        (chapter, version, purchase_id),
        &buyer_cookie,
        &author_cookie,
        &author_csrf,
    )
    .await;
    assert_eq!(
        fake.signed.load(Ordering::SeqCst),
        0,
        "alternate reader or progress routes must not sign G objects"
    );
    assert_eq!(
        scalar(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "alternate reader/progress routes must not record first grant"
    );
    // A chapter ciphertext UUID is not an image B membership, even for
    // its valid buyer. No fallback to public media or generic G signing.
    let asset_route = format!("/api/v1/reader/chapters/{chapter}/assets/{UUID}/delivery-grants");
    assert_eq!(
        send(
            &app,
            &asset_route,
            Some(&buyer_cookie),
            Some(&csrf),
            Some(ORIGIN),
            "{}"
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        send(
            &app,
            &asset_route,
            Some(&author_cookie),
            Some(&author_csrf),
            Some(ORIGIN),
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(&app, &asset_route, None, None, Some(ORIGIN), "{}")
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        send(
            &app,
            &asset_route,
            Some(&buyer_cookie),
            Some(&csrf),
            Some("https://hostile.invalid"),
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            &asset_route,
            Some(&buyer_cookie),
            Some(&csrf),
            Some(ORIGIN),
            r#"{"version_id":"999"}"#
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(
        fake.signed.load(Ordering::SeqCst),
        0,
        "forged asset grants must never reach G"
    );

    for invalid in [
        r#"{"version_id":"2"}"#,
        r#"{"bucket":"public"}"#,
        r#"{"object_key":"forged.enc"}"#,
        r#"{"purchase_id":"0"}"#,
    ] {
        assert_eq!(
            send(
                &app,
                &path,
                Some(&buyer_cookie),
                Some(&csrf),
                Some(ORIGIN),
                invalid,
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY,
        );
    }
    let success = send(
        &app,
        &path,
        Some(&buyer_cookie),
        Some(&csrf),
        Some(ORIGIN),
        &json!({"purchase_id":purchase_id.to_string()}).to_string(),
    )
    .await;
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(success.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(success.headers()["x-robots-tag"], "noindex");
    let body = json_body(success).await;
    assert_eq!(body["chapter_id"], chapter.to_string());
    assert_eq!(body["version_id"], version.to_string());
    assert_eq!(body["content"]["object_sha256"], sha);
    assert!(!body.to_string().contains(BODY));
    assert_eq!(fake.signed.load(Ordering::SeqCst), 1);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT chapter_id || ':' || version_id || ':' || object_kind
                 FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        format!("{chapter}:{version}:chapter"),
        "first successful signed direct V1 grant must create exact evidence"
    );
    let first_granted_at = pg(
        &url,
        &format!(
            "SELECT granted_at::TEXT FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
        ),
    );

    // A signed grant can be committed while its HTTP response gets lost.
    // It must NOT be silently called a completed browser opening.
    let open_route = format!("/api/v1/reader/chapters/{chapter}/open-confirmations");
    let open_body = json!({
        "purchase_id": purchase_id.to_string(), "version_id": version.to_string()
    }).to_string();
    assert_eq!(scalar(&url, &format!(
        "SELECT count(*) FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    )), 0, "issued grant != rendered chapter");
    assert_eq!(send(&app, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &open_body).await.status(), StatusCode::NOT_FOUND,
        "API-only grant router must not mount browser ACK");
    assert_eq!(send(&browser, &open_route, None, None, Some(ORIGIN),
        &open_body).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some("https://hostile.invalid"), &open_body).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&browser, &open_route, Some(&author_cookie), Some(&author_csrf),
        Some(ORIGIN), &open_body).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &json!({"purchase_id":purchase_id.to_string(),
                            "version_id":"99999999"}).to_string()).await.status(),
        StatusCode::FORBIDDEN);
    assert_eq!(send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &json!({"version_id":version.to_string(),
                            "source":"layer", "purchase_id":purchase_id.to_string()})
            .to_string()).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let opened = send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &open_body).await;
    assert_eq!(opened.status(), StatusCode::OK);
    assert_eq!(opened.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(opened).await["status"], "recorded");
    let first_open = pg(&url, &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    ));
    assert_eq!(send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &open_body).await.status(), StatusCode::OK);
    assert_eq!(pg(&url, &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    )), first_open, "render ACK replay does not alter first timestamp");
    assert_eq!(scalar(&url, &format!(
        "SELECT count(*) FROM reader_wallet_entries WHERE purchase_id={purchase_id}"
    )), 1, "render ACK cannot move money");

    let v2 = scalar(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter},2,'Second edition') RETURNING id"
        ),
    );
    pg(
        &url,
        &format!("UPDATE chapter_publications SET version_id={v2} WHERE chapter_id={chapter}"),
    );
    pg(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={book}"),
    );
    pg(
        &url,
        &format!("UPDATE chapters SET visibility='draft' WHERE id={chapter}"),
    );
    let hidden = send(
        &app,
        &path,
        Some(&buyer_cookie),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(hidden.status(), StatusCode::OK);
    assert_eq!(json_body(hidden).await["version_id"], version.to_string());
    assert_eq!(fake.signed.load(Ordering::SeqCst), 2);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT granted_at::TEXT FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        first_granted_at,
        "default source cannot overwrite a prior explicit direct first grant"
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        send(
            &app,
            &path,
            Some(&buyer_cookie),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    // The already-authorized request enters G signing before the refund.
    // Once D commits the reversal, C's final check must discard its bearer URL.
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    *fake.pause_next.lock().unwrap() = Some((entered_tx, release_rx));
    let race_app = app.clone();
    let race_path = path.clone();
    let race_session = buyer_cookie.clone();
    let race_csrf = csrf.clone();
    let in_flight = tokio::spawn(async move {
        send(
            &race_app,
            &race_path,
            Some(&race_session),
            Some(&race_csrf),
            Some(ORIGIN),
            "{}",
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered_rx)
        .await
        .expect("G must be reached before refund")
        .expect("signing barrier must remain open");
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) \
             VALUES({purchase_id},'ci:s034-http-refund','refund')"
        ),
    );
    release_tx
        .send(())
        .expect("resume signer after committed refund");
    let raced = tokio::time::timeout(Duration::from_secs(10), in_flight)
        .await
        .expect("no grant/refund deadlock")
        .expect("join grant handler");
    assert_eq!(raced.status(), StatusCode::FORBIDDEN);
    assert_eq!(raced.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(raced.into_body(), 8192).await.unwrap().is_empty());
    assert_eq!(
        fake.signed.load(Ordering::SeqCst),
        3,
        "G signed once, but C must discard the token after D revocation"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT granted_at::TEXT FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        first_granted_at,
        "failed final D recheck must not overwrite direct first grant"
    );
    // An ACK arriving after a refund/revocation cannot write new evidence.
    assert_eq!(send(&browser, &open_route, Some(&buyer_cookie), Some(&csrf),
        Some(ORIGIN), &open_body).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(pg(&url, &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    )), first_open);
    // A subsequent request is rejected by D before reaching G.
    assert_eq!(
        send(
            &app,
            &path,
            Some(&buyer_cookie),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(fake.signed.load(Ordering::SeqCst), 3);
    assert_eq!(
        read_page(&browser, &reader_path, Some(&buyer_cookie))
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    accounts
        .revoke_session(&session_token_digest(&buyer_cookie))
        .await
        .unwrap();
    assert_eq!(
        send(
            &app,
            &path,
            Some(&buyer_cookie),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
    );
    pool.close().await;
}
