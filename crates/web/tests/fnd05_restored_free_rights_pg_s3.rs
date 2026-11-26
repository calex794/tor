//! I-11/S061: exercise the saved FREE and Layer reader on the restored PG18 and PACK.
#![cfg(feature = "s3-free")]

use aws_sdk_s3::{
    config::{BehaviorVersion, Credentials, Region},
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{env, fs, num::NonZeroU32, process::Command};
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

const SOURCE: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci";
const RESTORED: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_offsite_restore_ci";
const ENDPOINT: &str = "http://127.0.0.1:9010";
const BUCKET: &str = "tor-dr-ci-book-pack";
const PHYSICAL: &str = "postgresql://tor_app@127.0.0.1:15436/tor_ci";
const V1: &str = "DROP-07 recovered historical FREE V1";

#[derive(Serialize, Deserialize)]
struct Rights {
    chapter: i64,
    version: i64,
    saved: String,
    buyer: String,
    revoked: String,
    unrelated: String,
}

fn isolated_url(key: &str, expected: &'static str) -> &'static str {
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_DR_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    assert!(env::var(key).is_ok_and(|url| url == expected));
    expected
}

fn pg(url: &str, sql: &str) -> String {
    let output = Command::new("psql")
        .args([
            "-XAtq",
            "--no-password",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "-c",
            sql,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "disposable PostgreSQL fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}

async fn actor(accounts: &PgAccounts, label: &str, hash: &str) -> (i64, String) {
    let session = generate_session_token();
    let user = accounts
        .register_with_session(
            &format!("drop07-{label}@example.invalid"),
            "Recovery test reader",
            hash,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (user, session)
}

#[tokio::test]
#[ignore = "seed inside the disposable sealed-remote-copy job"]
async fn seed_free_archive() {
    let url = isolated_url("TEST_DATABASE_URL", SOURCE);
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (book, owner, chapter, version): (i64, i64, i64, i64) = serde_json::from_str(&pg(
        url,
        "SELECT json_build_array(b.id,b.owner_user_id,c.id,p.version_id)
         FROM books b JOIN chapters c ON c.book_id=b.id JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE b.source_identity='test:drop07:backup'",
    ))
    .unwrap();
    let hash = hash_password("DROP07 disposable recovery fixture").unwrap();
    let owner_token = generate_session_token();
    let owner_digest = session_token_digest(&owner_token);
    pg(
        url,
        &format!("UPDATE users SET password_hash='{hash}' WHERE id={owner}"),
    );
    accounts
        .insert_session(owner, &owner_digest, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let (saved_user, saved) = actor(&accounts, "saved", &hash).await;
    let (buyer_id, buyer) = actor(&accounts, "buyer", &hash).await;
    let (revoked_id, revoked) = actor(&accounts, "revoked", &hash).await;
    let (_, unrelated) = actor(&accounts, "unrelated", &hash).await;
    let (moderator, moderator_token) = actor(&accounts, "moderator", &hash).await;
    pg(
        url,
        &format!(
            "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
                    VALUES({saved_user},{chapter},{version});
                  INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
                    VALUES({buyer_id},500,'RUB'),({revoked_id},500,'RUB');
                  INSERT INTO account_roles(user_id,role,is_active)
                    VALUES({moderator},'moderator',true)"
        ),
    );
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(id(owner), "Recovery Layer")
        .await
        .unwrap();
    let composition = layers
        .add_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    let offer = layers
        .publish_offer(id(owner), id(layer), 90, true)
        .await
        .unwrap();
    for (user, revoked) in [(buyer_id, false), (revoked_id, true)] {
        let term = layers
            .buy(&LayerCheckoutCommand {
                actor: id(user),
                wallet: WalletId::new(user).unwrap(),
                layer: id(layer),
                offer: id(offer),
                version: id(composition),
                expected_price: MoneyMinor::new(90).unwrap(),
                key: IdempotencyKey::parse(&generate_session_token()).unwrap(),
            })
            .await
            .unwrap();
        if revoked {
            pg(
                url,
                &format!(
                    "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
                           VALUES({},'legal_block','ci:drop07:layer-revoked')",
                    term.term_id
                ),
            );
        }
    }

    let author = PgAuthoring::new(pool.clone());
    let revision = author
        .ordinary_deletion_view(owner, book)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let requested = author
        .advance_ordinary_deletion(owner, book, revision, "start", &owner_digest, &hash)
        .await
        .unwrap();
    accounts
        .moderate_ordinary_deletion(
            &session_token_digest(&moderator_token),
            book,
            requested,
            "approve",
            "",
        )
        .await
        .unwrap();
    let rights = Rights {
        chapter,
        version,
        saved,
        buyer,
        revoked,
        unrelated,
    };
    fs::write(
        env::var_os("DROP07_RIGHTS_FILE").unwrap(),
        serde_json::to_vec(&rights).unwrap(),
    )
    .unwrap();
}

async fn read(
    client: &reqwest::Client,
    origin: &str,
    method: Method,
    route: &str,
    session: Option<&str>,
) -> Response {
    let mut request = client.request(method, format!("{origin}{route}"));
    if let Some(token) = session {
        let csrf = generate_session_token();
        request = request
            .header(
                header::COOKIE,
                format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
            )
            .header(header::ORIGIN, origin)
            .header("x-csrf-token", csrf);
    }
    let mut received = request.send().await.unwrap();
    let status = received.status();
    let headers = received.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = received.chunk().await.unwrap() {
        assert!(body.len() + chunk.len() <= 200_001);
        body.extend_from_slice(&chunk);
    }
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

async fn assert_v1(response: Response, version: i64) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-tor-free-version-id"],
        version.to_string()
    );
    assert_eq!(
        response.headers()["x-tor-free-sha256"],
        hex::encode(Sha256::digest(V1))
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        to_bytes(response.into_body(), 200_001)
            .await
            .unwrap()
            .as_ref(),
        V1.as_bytes()
    );
}

async fn assert_denial(response: Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    assert!(!response.headers().contains_key("x-tor-free-version-id"));
    let body = to_bytes(response.into_body(), 2_048).await.unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
#[ignore = "restore database and MinIO provisioned by the sealed-remote-copy job"]
async fn verify_restored_free_archive() {
    let url = isolated_url("DROP07_RESTORED_DATABASE_URL", RESTORED);
    verify_archive(url).await;
    println!("FND05_RESTORED_FREE_ARCHIVE_HTTP_OK");
}

#[tokio::test]
#[ignore = "only inside isolated physical PITR container"]
async fn verify_physical_free_archive() {
    assert!(std::path::Path::new("/.dockerenv").is_file());
    assert_eq!(
        env::var("TOR_I11_DISPOSABLE_CONTAINER").as_deref(),
        Ok("yes")
    );
    let url = isolated_url("DROP07_RESTORED_DATABASE_URL", PHYSICAL);
    verify_archive(url).await;
    println!("FND05_PHYSICAL_FREE_LAYER_HTTP_OK");
}

async fn verify_archive(url: &str) {
    let rights: Rights =
        serde_json::from_slice(&fs::read(env::var_os("DROP07_RIGHTS_FILE").unwrap()).unwrap())
            .unwrap();
    let catalog = PgCatalog::connect(url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let s3 = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_DR_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_DR_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "drop07-recovery-ci",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .build(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = tor_web::free_pack_http::router(accounts, false, origin.clone(), s3, BUCKET.into())
        .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let saved = format!(
        "/api/v1/chapters/{}/free-source?mode=saved&version_id={}",
        rights.chapter, rights.version
    );
    let layer = format!("/api/v1/reader/layer/free/{}/pack", rights.chapter);
    assert_v1(
        read(&client, &origin, Method::GET, &saved, Some(&rights.saved)).await,
        rights.version,
    )
    .await;
    assert_v1(
        read(&client, &origin, Method::POST, &layer, Some(&rights.buyer)).await,
        rights.version,
    )
    .await;
    for token in [
        None,
        Some(rights.unrelated.as_str()),
        Some(rights.buyer.as_str()),
    ] {
        assert_denial(
            read(&client, &origin, Method::GET, &saved, token).await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    assert_denial(
        read(&client, &origin, Method::POST, &layer, None).await,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    for token in [&rights.unrelated, &rights.revoked, &rights.saved] {
        assert_denial(
            read(&client, &origin, Method::POST, &layer, Some(token)).await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let public = format!("/api/v1/chapters/{}/free-source", rights.chapter);
    assert_denial(
        read(&client, &origin, Method::GET, &public, None).await,
        StatusCode::NOT_FOUND,
    )
    .await;
    server.abort();
}
