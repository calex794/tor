//! D-05 / #1022: actual PG18, MinIO and authenticated Layer FREE SINGLE
//! grant. Runs only against the disposable tor_ci and loopback S3 from CI.
#![cfg(feature = "s3-free")]
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    response::Response,
    Router,
};
use sha2::{Digest, Sha256};
use std::{env, num::NonZeroU32, process::Command};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::{postgres::PgAuthoring, PgCatalog};
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{upload_and_verify, FreeChapterObject},
};
use tower::ServiceExt;

#[cfg(feature = "s3-paid")]
#[path = "layer_free_private_s3_pg/four_live.rs"]
mod four_live;
#[path = "layer_free_private_s3_pg/live_chromium.rs"]
mod live_chromium;

const ORIGIN: &str = "http://127.0.0.1:3098";
const ENDPOINT: &str = "http://127.0.0.1:9000";
const BUCKET: &str = "tor-layer-free-archive-ci";
const TEXT: &str = "Layer historical FREE V1 \u{0433}\u{043b}\u{0430}\u{0432}\u{0430} <script>globalThis.__layerFreeXss=1</script>";

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:"));
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
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
        .expect("disposable PostgreSQL fixture");
    assert!(
        output.status.success(),
        "D-05 fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn user(accounts: &PgAccounts, role: &str) -> (i64, String, String) {
    let token = generate_session_token();
    let email = format!(
        "d05-layer-free-{role}-{}@example.invalid",
        token[..12].to_ascii_lowercase()
    );
    let password = hash_password("D05 Layer Free Archive Strict Password 2026").unwrap();
    let actor = accounts
        .register_with_session(
            &email,
            "D05 archive fixture",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (actor, token, password)
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    session: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
) -> Response {
    let mut request = Request::builder().uri(path).method(method);
    if let Some(session) = session {
        let cookie = match csrf {
            Some(csrf) => format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
            None => format!("tor-dev-session={session}"),
        };
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PG18/MinIO, single-thread CI"]
async fn d05_layer_free_single_historical_buyer_signed_read_and_denials() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"),
        "must not run on production"
    );
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "layer-free-disposable-minio",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, owner_session, owner_hash) = user(&accounts, "owner").await;
    let (buyer, buyer_session, _) = user(&accounts, "buyer").await;
    let (_, unrelated_session, _) = user(&accounts, "unrelated").await;
    let (moderator, moderator_session, _) = user(&accounts, "moderator").await;
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role,is_active) VALUES({moderator},'moderator',true)"),
    );
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:d05:archive-free:{owner}','ru','D05 archival FREE S3','public',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Archived FREE V1','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(TEXT.as_bytes()).into();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
             VALUES({chapter},1,decode('{}','hex')) RETURNING id",
            hex::encode(digest),
        ),
    )
    .parse()
    .unwrap();
    let object = FreeChapterObject::for_markdown(chapter, version, TEXT).unwrap();
    upload_and_verify(&store, &object, TEXT).await.unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO free_chapter_storage_objects
             (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
             VALUES({chapter},{version},'{BUCKET}','{}',decode('{}','hex'),{})",
            object.key(),
            hex::encode(object.sha256()),
            object.size_bytes(),
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );

    #[cfg(feature = "s3-paid")]
    let four_editions = four_live::install(&url, owner, book, &accounts, &store, &client).await;

    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(id(owner), "Archived FREE test")
        .await
        .unwrap();
    let term_version = layers
        .add_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    let offer = layers
        .publish_offer(id(owner), id(layer), 90, true)
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},300,'RUB')"
        ),
    );
    let purchased = layers
        .buy(&LayerCheckoutCommand {
            actor: id(buyer),
            wallet: WalletId::new(buyer).unwrap(),
            layer: id(layer),
            offer: id(offer),
            version: id(term_version),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "F".repeat(42))).unwrap(),
        })
        .await
        .unwrap();

    // Genuine authenticated author deletion and independent site moderation.
    let author = PgAuthoring::new(pool.clone());
    let revision = author
        .ordinary_deletion_view(owner, book)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let pending = author
        .advance_ordinary_deletion(
            owner,
            book,
            revision,
            "start",
            &session_token_digest(&owner_session),
            &owner_hash,
        )
        .await
        .unwrap();
    accounts
        .moderate_ordinary_deletion(
            &session_token_digest(&moderator_session),
            book,
            pending,
            "approve",
            "",
        )
        .await
        .unwrap();

    // This branch's private capability is an explicit operator opt-in.
    env::set_var("TOR_LAYER_FREE_ARCHIVE_ENABLED", "1");
    let free_app = tor_web::free_reader_http::router(
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        store,
        BUCKET.to_owned(),
        ENDPOINT.to_owned(),
    )
    .unwrap();
    env::remove_var("TOR_LAYER_FREE_ARCHIVE_ENABLED");
    #[cfg(feature = "s3-paid")]
    let app = free_app.merge(four_live::paid_router(&url, &accounts, &client).await);
    #[cfg(not(feature = "s3-paid"))]
    let app = free_app;

    let page = format!("/read/layer/free/{chapter}");
    let get_old = call(&app, Method::GET, &page, Some(&buyer_session), None, None).await;
    assert_eq!(get_old.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(get_old.into_body(), 250_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("data-layer-free-reader"));
    assert!(!html.contains("X-Amz-Signature"));
    assert!(!html.contains("private/free-chapters/"));
    assert_eq!(
        call(
            &app,
            Method::GET,
            &page,
            Some(&unrelated_session),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        call(&app, Method::GET, &page, None, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );

    let csrf = generate_session_token();
    let grant_path = format!("/api/v1/reader/layer/free/{chapter}/grant");
    assert_eq!(
        call(
            &app,
            Method::POST,
            &grant_path,
            Some(&buyer_session),
            Some(&csrf),
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            &grant_path,
            Some(&unrelated_session),
            Some(&csrf),
            Some(ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    let granted = call(
        &app,
        Method::POST,
        &grant_path,
        Some(&buyer_session),
        Some(&csrf),
        Some(ORIGIN),
    )
    .await;
    assert_eq!(granted.status(), StatusCode::OK);
    let data = to_bytes(granted.into_body(), 200_000).await.unwrap();
    let grant: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(grant["schema"], "tor.layer-free-s3-grant.v1");
    assert_eq!(grant["chapter_id"], chapter.to_string());
    assert_eq!(grant["version_id"], version.to_string());
    let signed = grant["url"].as_str().unwrap();
    assert!(signed.starts_with(ENDPOINT));
    assert!(signed.contains(&format!("private/free-chapters/{chapter}/{version}.md")));
    let opened = reqwest::Client::new().get(signed).send().await.unwrap();
    assert_eq!(opened.status().as_u16(), 200);
    assert_eq!(opened.bytes().await.unwrap().as_ref(), TEXT.as_bytes());

    // Browser executes the exact Rust router, actual PG entitlement and
    // actual signed MinIO GET. No synthetic response replay is accepted.
    // Both viewports must render literal <script> text without executing it.
    live_chromium::verify(
        &app,
        chapter,
        version,
        &buyer_session,
        &unrelated_session,
        TEXT,
    )
    .await;

    #[cfg(feature = "s3-paid")]
    four_live::verify(
        &app,
        (chapter, version),
        four_editions,
        &buyer_session,
        &unrelated_session,
    )
    .await;

    pg(
        &url,
        &format!(
            "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
             VALUES({},'legal_block','ci:d05-archive-free-revoke:{}')",
            purchased.term_id, purchased.term_id,
        ),
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            &grant_path,
            Some(&buyer_session),
            Some(&csrf),
            Some(ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
}
