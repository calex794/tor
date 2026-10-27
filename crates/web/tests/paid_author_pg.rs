//! B/G/H: real owner browser Save -> private MinIO -> PG attached paid
//! chapter, without a public paid plaintext route or a second release step.
#![cfg(feature = "s3-paid")]

use async_trait::async_trait;
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{env, num::NonZeroU32, process::Command, sync::Arc, time::Duration};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{paid_storage::PgPaidStorage, PgCatalog},
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_contracts::paid_delivery::{
    ChapterVersionPort, ChapterVersionRef, ObjectRef, PositiveId, PrivateObjectDeliveryPort,
    SecretValue,
};
use tor_reading::repositories::PgReader;
use tor_storage::{
    domain::provider::StorageError,
    repositories::{paid_delivery::PgPrivateDelivery, s3::S3PrivateStore},
    services::{
        crypto::{ContentKek, ContentKeyring},
        provider::PrivateObjectStore,
    },
};
use tor_web::PgPaidAuthorPublisher;
use tower::ServiceExt;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const ORIGIN: &str = "http://127.0.0.1:3098";
const BUCKET: &str = "tor-private-author-creation-ci";

/// CI failure injection: no private S3 upload is possible; B must remain
/// hidden and a replay with the original command key must recover safely.
struct UnavailableStore;

#[async_trait]
impl PrivateObjectStore for UnavailableStore {
    async fn put_immutable(&self, _object: &ObjectRef, _bytes: &[u8]) -> Result<(), StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn head_verified(&self, _object: &ObjectRef) -> Result<bool, StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn get_verified(&self, _object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn presign_verified(
        &self,
        _object: &ObjectRef,
        _ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        Err(StorageError::Unavailable)
    }
}

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let result = Command::new("psql")
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
        .expect("psql client required by isolated PG test");
    assert!(
        result.status.success(),
        "disposable fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn user(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let token = generate_session_token();
    let pass = hash_password("PaidAuthorPG18Owned2026").unwrap();
    let id = accounts
        .register_with_session(
            email,
            "Paid author CI",
            &pass,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, token)
}

fn cookie(session: &str, csrf: &str) -> String {
    format!("tor-dev-session={session}; tor-dev-csrf={csrf}")
}

async fn browse(app: &Router, path: &str, session: &str, csrf: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie(session, csrf))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn send(
    app: &Router,
    book: i64,
    session: &str,
    csrf: &str,
    command: &str,
    valid_csrf: bool,
    title: &str,
) -> StatusCode {
    let form = format!(
        "number=1&title={title}&body=Paid%20chapter%20V2&access=paid&price_amount=100&command_key={command}&csrf_token={}",
        if valid_csrf { csrf } else { "wrong-csrf" }
    );
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/me/books/{book}/chapters"))
                .method("POST")
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie(session, csrf))
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated local MinIO"]
async fn paid_author_post_creates_only_verified_v2_and_replays_without_second_release() {
    let url = env::var("TEST_DATABASE_URL").expect("CI database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "local-paid-author",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    let store = S3PrivateStore::new(client, BUCKET.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, owner_session) = user(&accounts, "paid-author-v2-ci-owner@example.invalid").await;
    let (_, other_session) = user(&accounts, "paid-author-v2-ci-other@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:real-paid-author-save','ru','Paid author creation','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let ring = Arc::new(
        ContentKeyring::with_v2(
            Arc::new(ContentKek::from_bytes([0x41; 32])),
            Arc::new(ContentKek::from_bytes([0x42; 32])),
        )
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap(),
    );
    let signer: Arc<dyn PrivateObjectDeliveryPort> = Arc::new(
        PgPrivateDelivery::with_keyring(
            pool.clone(),
            store.clone(),
            ring.clone(),
            BUCKET.to_owned(),
        )
        .unwrap(),
    );
    let grants_only = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        (signer.clone(), ENDPOINT.to_owned()),
        None,
    )
    .unwrap();
    let csrf = generate_session_token();
    let command = generate_session_token();
    assert_eq!(
        send(
            &grants_only,
            book,
            &owner_session,
            &csrf,
            &command,
            true,
            "Paid"
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
        "paid reader must not automatically enable paid author writes"
    );
    let (status, page) = browse(
        &grants_only,
        &format!("/me/books/{book}/chapters/new"),
        &owner_session,
        &csrf,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!page.contains("<option value=\"paid\">"));

    let author = PgPaidAuthorPublisher::new(
        catalog.clone(),
        store.clone(),
        ring.clone(),
        BUCKET.to_owned(),
    )
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        (false, ORIGIN.to_owned()),
        (signer.clone(), ENDPOINT.to_owned()),
        None,
        author,
    )
    .unwrap();
    let (status, page) = browse(
        &app,
        &format!("/me/books/{book}/chapters/new"),
        &owner_session,
        &csrf,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<option value=\"paid\">"));
    assert_eq!(
        send(&app, book, &owner_session, &csrf, &command, false, "Paid").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, book, &other_session, &csrf, &command, true, "Paid").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, book, &owner_session, &csrf, &command, true, "Paid").await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, book, &owner_session, &csrf, &command, true, "Paid").await,
        StatusCode::SEE_OTHER,
        "identical POST replay must be successful without another publication"
    );
    assert_eq!(
        send(
            &app,
            book,
            &owner_session,
            &csrf,
            &command,
            true,
            "Different"
        )
        .await,
        StatusCode::CONFLICT,
        "same key with altered text/metadata must never replace the version"
    );

    let result = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || v.id || '|' || c.visibility || '|' || c.access_level || '|' || o.kek_id
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id
             JOIN storage_objects o ON o.id=v.storage_object_id
             WHERE c.book_id={book}"
        ),
    );
    let values = result.split('|').collect::<Vec<_>>();
    assert_eq!(values.len(), 5);
    let chapter_id: i64 = values[0].parse().unwrap();
    let version_id: i64 = values[1].parse().unwrap();
    assert_eq!(values[2], "public");
    assert_eq!(values[3], "paid");
    assert_eq!(values[4], "content-kek-v2");
    let version_count: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter_id}"),
    )
    .parse()
    .unwrap();
    assert_eq!(version_count, 1);
    let pinned = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter_id).unwrap(),
        version_id: PositiveId::new(version_id).unwrap(),
    };
    let encrypted = PgPaidStorage::new(pool.clone())
        .encrypted_chapter(pinned)
        .await
        .unwrap()
        .unwrap();
    let private_bytes = store.get_verified(&encrypted.object).await.unwrap();
    assert!(!private_bytes
        .windows(b"Paid chapter V2".len())
        .any(|w| w == b"Paid chapter V2"));
    assert!(signer
        .grant_verified_object(&encrypted, Duration::from_secs(90))
        .await
        .is_ok());
    // Simulate a failed private upload. An owner-visible draft is durable,
    // but readers see no published pointer until a replay verifies real S3.
    let outage_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:paid-author-outage','ru','Private outage','public',{owner}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let outage_key = generate_session_token();
    let draft = save_chapter(SaveChapterRequest {
        book_id: outage_book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Retry",
        body: "Text must never leak after a failed upload",
        access: "paid",
        price_amount: 100,
        command_key: &outage_key,
    })
    .unwrap();
    let failed = PgPaidAuthorPublisher::new(
        PgCatalog::new(pool.clone()),
        UnavailableStore,
        ring.clone(),
        BUCKET.to_owned(),
    )
    .unwrap();
    assert_eq!(
        failed.publish(owner, &draft).await,
        Err(AuthorError::Unavailable)
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapters c JOIN chapter_publications p
                 ON p.chapter_id=c.id WHERE c.book_id={outage_book}"
            ),
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility FROM chapters WHERE book_id={outage_book}"),
        ),
        "draft"
    );
    let recovered = PgPaidAuthorPublisher::new(
        PgCatalog::new(pool.clone()),
        store.clone(),
        ring,
        BUCKET.to_owned(),
    )
    .unwrap();
    recovered.publish(owner, &draft).await.unwrap();
    recovered.publish(owner, &draft).await.unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapters c JOIN chapter_publications p
                 ON p.chapter_id=c.id WHERE c.book_id={outage_book}"
            ),
        ),
        "1",
        "recovery must publish once, even when the response is retried"
    );
    pool.close().await;
}
