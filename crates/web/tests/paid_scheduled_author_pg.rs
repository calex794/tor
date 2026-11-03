//! S058: real paid book-level slot, private MinIO and author HTTP.
//! No production connections, worker activation, or schema migrations.
#![cfg(feature = "s3-paid")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use sqlx::PgPool;
use std::{env, num::NonZeroU32, sync::Arc, time::Duration};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::{
    postgres::{
        book_delayed_schedule::{BookSlotFrontier, BookSlotReleaseOutcome},
        PgAuthoring,
    },
    PgCatalog,
};
use tor_contracts::paid_delivery::PrivateObjectDeliveryPort;
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{paid_delivery::PgPrivateDelivery, s3::S3PrivateStore},
    services::crypto::{ContentKek, ContentKeyring},
};
use tor_web::PgPaidAuthorPublisher;
use tower::ServiceExt;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const ORIGIN: &str = "http://127.0.0.1:3098";
const BUCKET: &str = "tor-s058-scheduled-author-ci";
const SECRET: &str = "S058_UNPUBLISHED_PAID_BODY_SECRET_2026";

async fn request_body(app: &Router, path: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_paid(
    app: &Router,
    book: i64,
    session: &str,
    csrf: &str,
    command: &str,
    delayed: bool,
) -> StatusCode {
    let delayed_field = if delayed {
        "&is_delayed_publication=1"
    } else {
        ""
    };
    let form = format!(
        "number=1&title=S058%20scheduled%20paid&body={SECRET}&access=paid&price_amount=175&command_key={command}&csrf_token={csrf}{delayed_field}"
    );
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/me/books/{book}/chapters"))
                .method("POST")
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(
                    header::COOKIE,
                    format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
                )
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
#[ignore = "requires disposable PG18 tor_ci and local MinIO"]
async fn s058_paid_private_browser_staging_and_exact_book_slot_release() {
    let url = env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
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
            "s058-paid-scheduled",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    let store = S3PrivateStore::new(client, BUCKET.to_owned()).unwrap();

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool: PgPool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner_session = generate_session_token();
    let pass = hash_password("S058PaidAuthorCI_2026_LongPassword").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("s058-paid-scheduled-{nonce}@example.invalid"),
            "S058 paid author",
            &pass,
            &session_token_digest(&owner_session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru',$2,'public',$3) RETURNING id",
    )
    .bind(format!("ci:s058-paid-author:{nonce}"))
    .bind(format!("S058 scheduled paid book {nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();

    let author = PgAuthoring::new(pool.clone());
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &["09:00".to_owned()])
        .await
        .unwrap();
    let keyring = Arc::new(
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
            keyring.clone(),
            BUCKET.to_owned(),
        )
        .unwrap(),
    );
    let publisher =
        PgPaidAuthorPublisher::new(catalog.clone(), store, keyring, BUCKET.to_owned()).unwrap();
    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        (false, ORIGIN.to_owned()),
        (signer, ENDPOINT.to_owned()),
        None,
        publisher,
    )
    .unwrap();
    let csrf = generate_session_token();
    let command = generate_session_token();

    assert_eq!(
        post_paid(&app, book, &owner_session, &csrf, &command, true).await,
        StatusCode::SEE_OTHER,
        "author must be able to privately stage a paid book-scheduled chapter"
    );
    assert_eq!(
        post_paid(&app, book, &owner_session, &csrf, &command, true).await,
        StatusCode::SEE_OTHER,
        "retrying delayed save may not create another edition"
    );
    assert_eq!(
        post_paid(&app, book, &owner_session, &csrf, &command, false).await,
        StatusCode::CONFLICT,
        "the delayed command key cannot change into immediate publication"
    );

    let staged: (i64, i64, String, String, i64, String) = sqlx::query_as(
        "SELECT c.id,v.id,c.visibility,c.access_level,c.price_amount,o.state
         FROM chapters c JOIN chapter_versions v ON v.chapter_id=c.id
         JOIN storage_objects o ON o.id=v.storage_object_id
         JOIN chapter_reader_manifests m ON m.chapter_id=c.id
           AND m.version_id=v.id AND m.storage_object_id=o.id
         WHERE c.book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (chapter, version, visibility, access, price, state) = staged;
    assert_eq!(
        (visibility.as_str(), access.as_str(), price, state.as_str()),
        ("draft", "paid", 175, "attached")
    );
    let count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_before, 0,
        "private ciphertext is not reader publication"
    );
    let (_, before) = request_body(&app, &format!("/books/{book}")).await;
    assert!(
        !before.contains(SECRET),
        "plaintext must never be in book HTML"
    );
    assert!(
        !before.contains("S058 scheduled paid"),
        "draft TOC must not expose chapter title"
    );

    // Make the specific book's next slot due. No worker is activated.
    sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=clock_timestamp()-interval '1 second' WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let token = generate_session_token();
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    let still_private: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_private, 0);
    assert_eq!(
        author.release_bound_book_slot(&claim, &token).await,
        Ok(BookSlotReleaseOutcome::Released {
            chapter_id: chapter,
            version_id: version
        })
    );
    assert!(author
        .release_bound_book_slot(&claim, &token)
        .await
        .is_err());
    let published: (String, String, i64, i64) = sqlx::query_as(
        "SELECT c.visibility,c.access_level,c.price_amount,p.version_id
         FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        published,
        ("public".to_owned(), "paid".to_owned(), 175, version)
    );
    let (first, encrypted, outbox): (i64, i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1),
          (SELECT count(*) FROM chapter_encrypted_release_events WHERE chapter_id=$1),
          (SELECT count(*) FROM outbox_events
           WHERE dedupe_key=$2)",
    )
    .bind(chapter)
    .bind(format!("chapter-published:{chapter}:{version}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((first, encrypted, outbox), (1, 1, 1));
    let (_, after) = request_body(&app, &format!("/books/{book}")).await;
    assert!(
        !after.contains(SECRET),
        "published paid HTML must not reveal plaintext"
    );
    pool.close().await;
}
