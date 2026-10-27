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
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{
        paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry, s3::S3PrivateStore,
    },
    services::{
        crypto::{seal_chapter, ContentKek},
        provider::upload_and_verify,
    },
};
use tower::ServiceExt;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const BUCKET: &str = "tor-private-chapter-ci";
const ORIGIN: &str = "http://127.0.0.1:3098";
const OBJECT_ID: &str = "f0c0e034-2026-4034-8034-000000000034";
const MARKDOWN: &str = "Оплаченная V1: кириллица А😀Б\n\n第二章 — только купленная редакция";

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

async fn decode(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap()
}

fn save_ephemeral_fixture(grant: Value, version: i64) {
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
        "expected_markdown": MARKDOWN,
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
    let (_stranger, stranger_cookie) = user(&accounts, "s034-minio-stranger@tor-ci.example").await;
    let csrf = generate_session_token();

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
             VALUES('ci:s034-private-e2e','ru','MinIO paid edition','public',{author}) \
             RETURNING id"
        ),
    );
    let chapter = scalar(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},1,'Secret edition V1','public','paid',175) RETURNING id"
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

    let signer = PgPrivateDelivery::new(pool.clone(), s3, kek, BUCKET.to_owned()).unwrap();
    let app = tor_web::router_with_postgres_reading_auth_paid_grants(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
        Arc::new(signer),
        None,
    );
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
        post_grant(&app, chapter, None, None, ORIGIN, "{}")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

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
    assert_eq!(first["schema"], "tor.paid_delivery.v1");
    assert!(!first.to_string().contains(MARKDOWN), "no plaintext in API");

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
    pg(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={book}"),
    );
    pg(
        &url,
        &format!("UPDATE chapters SET visibility='draft' WHERE id={chapter}"),
    );
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
    let still_v1 = decode(preserved).await;
    assert_eq!(still_v1["version_id"], v1.to_string());
    assert!(!still_v1.to_string().contains(MARKDOWN));
    save_ephemeral_fixture(still_v1, v1);

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
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
    // Previous signed links remain bearer credentials until TTL; the Node
    // follow-up consumes this just-issued URL and validates its real S3 GET.
    pool.close().await;
}
