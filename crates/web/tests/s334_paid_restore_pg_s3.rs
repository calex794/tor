//! S3-34 / I-11: loss-and-restore exercise for a purchased historical V1.
//! Only GitHub Actions' disposable PostgreSQL and loopback private MinIO.
#![cfg(feature = "s3-paid")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
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
use std::{env, fs, fs::OpenOptions, io::Write, num::NonZeroU32, path::PathBuf, sync::Arc};
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
    ChapterVersionRef, ObjectId, ObjectRef, PositiveId, PrivateObjectDeliveryPort,
};
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{
        paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry, s3::S3PrivateStore,
    },
    services::{
        crypto::{seal_chapter, sha256_bytes, ContentKek, CryptoError},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};
use tower::ServiceExt;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const BUCKET: &str = "tor-private-chapter-ci";
const BACKUP_BUCKET: &str = "tor-s334-backup-ci";
const ORIGIN: &str = "http://127.0.0.1:3098";
const ID: &str = "f0c0e034-2026-4034-8034-000000000134";
const MARKDOWN: &str = "S334 купленная редакция V1: А😀Б и 第二章";
const SOURCE: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci";
const RESTORED: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_s334_restore_ci";

fn require_ci(restored: bool) -> &'static str {
    assert_eq!(env::var("GITHUB_ACTIONS").unwrap(), "true");
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    assert_eq!(env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(), "tor-chapter-ci-user");
    assert_eq!(
        env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
        "tor-chapter-ci-secret-only"
    );
    let url = env::var(if restored {
        "S334_RESTORED_DATABASE_URL"
    } else {
        "TEST_DATABASE_URL"
    })
    .unwrap();
    let expected = if restored { RESTORED } else { SOURCE };
    assert_eq!(url, expected, "forbid production database in recovery test");
    expected
}

fn ci_file(name: &str) -> PathBuf {
    let runner = PathBuf::from(env::var("RUNNER_TEMP").expect("CI temporary directory"));
    assert!(runner.is_absolute());
    runner.join(name)
}

fn key() -> Arc<ContentKek> {
    let data = fs::read(ci_file("s334-content-kek.bin")).expect("offline KEK must survive");
    let bytes: [u8; 32] = data.try_into().expect("KEK is exactly 32 bytes");
    Arc::new(ContentKek::from_bytes(bytes))
}

fn client() -> Client {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "s334-loopback-only",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    Client::from_conf(config)
}

async fn register(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let session = generate_session_token();
    let hash = hash_password("S334IsolationOnly2026SecurePassword").unwrap();
    let id = accounts
        .register_with_session(
            email,
            "S334 recovery",
            &hash,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, session)
}

async fn grant(
    app: &Router,
    chapter: i64,
    session: Option<&str>,
    csrf: Option<&str>,
    body: &str,
) -> Response {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/reader/chapters/{chapter}/delivery-grants"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN);
    if let Some(session) = session {
        req = req.header(
            header::COOKIE,
            format!(
                "tor-dev-session={session}; tor-dev-csrf={}",
                csrf.unwrap_or("A")
            ),
        );
    }
    if let Some(csrf) = csrf {
        req = req.header("x-csrf-token", csrf);
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn app(url: &str, client: Client, kek: Arc<ContentKek>) -> Router {
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let store = S3PrivateStore::new(client, BUCKET.to_owned()).unwrap();
    let signer = PgPrivateDelivery::new(pool, store, kek, BUCKET.to_owned()).unwrap();
    tor_web::router_with_postgres_reading_auth_paid_grants(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
        Arc::new(signer),
        None,
    )
}

async fn decoded(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap()
}

fn save_json(path: PathBuf, value: &Value) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).unwrap();
    file.write_all(value.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
}

fn state() -> Value {
    serde_json::from_slice(&fs::read(ci_file("s334-state.json")).unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "isolated GitHub Actions tor_ci + MinIO only"]
async fn s3_34_seed_paid_v1_and_publish_v2() {
    let url = require_ci(false);
    let c = client();
    c.create_bucket().bucket(BUCKET).send().await.unwrap();
    let kek = key();

    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (author, _) = register(&accounts, "s334-author@tor-ci.example").await;
    let (buyer, buyer_cookie) = register(&accounts, "s334-buyer@tor-ci.example").await;
    let (_stranger, stranger_cookie) = register(&accounts, "s334-stranger@tor-ci.example").await;
    let csrf = generate_session_token();

    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,600,'RUB')",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s334-paid-restore','ru','S334 paid V1 recovery','public',$1)
         RETURNING id",
    )
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'S334 encrypted chapter','public','paid',175) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter)
    .bind(MARKDOWN)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();

    let pinned = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(v1).unwrap(),
    };
    let object_id = ObjectId::parse(ID).unwrap();
    let sealed = seal_chapter(pinned, object_id.clone(), MARKDOWN, &kek).unwrap();
    let store = S3PrivateStore::new(c.clone(), BUCKET.to_owned()).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let reference = registry
        .reserve_pending(&sealed, &kek, BUCKET)
        .await
        .unwrap();
    let receipt = upload_and_verify(&store, &reference, &sealed.ciphertext)
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
    let purchase = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                PositiveId::new(buyer).unwrap(),
                WalletId::new(buyer).unwrap(),
                pinned,
                MoneyMinor::new(175).unwrap(),
                IdempotencyKey::parse(&format!("{}4", "S".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'New V2 not purchased') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$1 WHERE chapter_id=$2")
        .bind(v2)
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    let a = app(url, c, kek).await;
    let response = grant(&a, chapter, Some(&buyer_cookie), Some(&csrf), "{}").await;
    assert_eq!(response.status(), StatusCode::OK);
    let grant_data = decoded(response).await;
    assert_eq!(grant_data["version_id"], v1.to_string());
    assert!(!grant_data.to_string().contains(MARKDOWN));
    assert_eq!(
        grant(&a, chapter, Some(&stranger_cookie), Some(&csrf), "{}")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let row: (i64, String) = sqlx::query_as(
        "SELECT v.id, s.state
         FROM chapter_versions v JOIN storage_objects s ON s.id=v.storage_object_id
         WHERE v.id=$1",
    )
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (v1, "attached".to_string()));
    save_json(
        ci_file("s334-state.json"),
        &json!({
            "chapter_id": chapter,
            "v1_id": v1,
            "v2_id": v2,
            "purchase_id": purchase.purchase_id.get(),
            "buyer_cookie": buyer_cookie,
            "stranger_cookie": stranger_cookie,
            "csrf": csrf,
            "sha256_ciphertext": hex::encode(reference.sha256_ciphertext),
        }),
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "isolated GitHub Actions restored DB and MinIO only"]
async fn s3_34_verify_restored_paid_v1_and_rejected_keys() {
    let url = require_ci(true);
    let snapshot = state();
    let chapter = snapshot["chapter_id"].as_i64().unwrap();
    let v1 = snapshot["v1_id"].as_i64().unwrap();
    let v2 = snapshot["v2_id"].as_i64().unwrap();
    let buyer = snapshot["buyer_cookie"].as_str().unwrap();
    let stranger = snapshot["stranger_cookie"].as_str().unwrap();
    let csrf = snapshot["csrf"].as_str().unwrap();
    let purchase_id = snapshot["purchase_id"].as_i64().unwrap();
    let digest = snapshot["sha256_ciphertext"].as_str().unwrap();
    let object_id = ObjectId::parse(ID).unwrap();
    let c = client();
    let kek = key();
    let store = S3PrivateStore::new(c.clone(), BUCKET.to_string()).unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    let restored: i64 = sqlx::query_scalar(
        "SELECT id FROM chapter_versions WHERE id=$1 AND storage_object_id=$2::uuid",
    )
    .bind(v1)
    .bind(ID)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(restored, v1);
    let current: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, v2, "restored published V2 must not erase bought V1");

    // The source S3 object was deleted after backup. Neither PostgreSQL nor
    // an intact recovery KEK alone permits issuing a grant for missing bytes.
    let missing = app(url, c.clone(), key()).await;
    assert_eq!(
        grant(&missing, chapter, Some(buyer), Some(csrf), "{}")
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    // Recover the exact historical ciphertext from a different private
    // backup namespace, preserving metadata needed for a verified S3 HEAD.
    let backup_key = format!("s334/private/chapter-versions/{ID}.enc");
    let response = c
        .get_object()
        .bucket(BACKUP_BUCKET)
        .key(&backup_key)
        .send()
        .await
        .unwrap();
    let bytes = response.body.collect().await.unwrap().into_bytes().to_vec();
    let expected = hex::decode(digest).unwrap();
    assert_eq!(sha256_bytes(&bytes).as_slice(), expected.as_slice());
    c.put_object()
        .bucket(BUCKET)
        .key(format!("private/chapter-versions/{ID}.enc"))
        .content_type("application/octet-stream")
        .metadata("sha256", digest)
        .if_none_match("*")
        .body(ByteStream::from(bytes))
        .send()
        .await
        .unwrap();
    let reference: ObjectRef = tor_contracts::paid_delivery::ObjectRef::new(
        object_id,
        tor_contracts::paid_delivery::EncryptedObjectKind::Chapter,
        expected.try_into().unwrap(),
        sqlx::query_scalar::<_, i64>(
            "SELECT size_bytes FROM storage_objects WHERE id=$1::uuid",
        )
        .bind(ID)
        .fetch_one(&pool)
        .await
        .unwrap() as u64,
    )
    .unwrap();
    assert!(store.get_verified(&reference).await.is_ok());

    // The KEK must be independently recoverable, never inferred from PG or
    // private S3 data. Missing or wrong KEK must deny all new grants.
    assert!(matches!(
        ContentKek::from_environment(),
        Err(CryptoError::UnavailableKey)
    ));
    let wrong = app(url, c.clone(), Arc::new(ContentKek::from_bytes([99_u8; 32]))).await;
    assert_eq!(
        grant(&wrong, chapter, Some(buyer), Some(csrf), "{}")
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    let correct = app(url, c, kek).await;
    assert_eq!(
        grant(&correct, chapter, None, None, "{}")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        grant(&correct, chapter, Some(stranger), Some(csrf), "{}")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        grant(
            &correct,
            chapter,
            Some(stranger),
            Some(csrf),
            &json!({"purchase_id":purchase_id.to_string()}).to_string(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "a stolen purchase ID cannot grant a V1 after disaster recovery"
    );
    let accepted = grant(&correct, chapter, Some(buyer), Some(csrf), "{}").await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(accepted.headers()[header::CACHE_CONTROL], "private, no-store");
    let authorized = decoded(accepted).await;
    assert_eq!(authorized["version_id"], v1.to_string());
    assert_eq!(authorized["content"]["object_sha256"], digest);
    assert!(!authorized.to_string().contains(MARKDOWN));
    save_json(
        ci_file("s334-webcrypto.json"),
        &json!({
            "grant": authorized,
            "purchased_version_id": v1.to_string(),
            "expected_markdown": MARKDOWN,
        }),
    );

    // Refund AFTER issuing a test grant: subsequent grants must be denied.
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'ci:s334-refund-after-restore','refund')",
    )
    .bind(purchase_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        grant(&correct, chapter, Some(buyer), Some(csrf), "{}")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    pool.close().await;
}
