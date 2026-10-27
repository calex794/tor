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
        Arc,
    },
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
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) \
             VALUES({purchase_id},'ci:s034-http-refund','refund')"
        ),
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
    assert_eq!(fake.signed.load(Ordering::SeqCst), 2);
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
