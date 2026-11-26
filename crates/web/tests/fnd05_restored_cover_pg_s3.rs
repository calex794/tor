//! FND-05/S061/S025: restore an approved cover using only the retained
//! PostgreSQL snapshot and the two independently inventoried WebP objects.
//! All three buckets are distinct disposable buckets on the CI vault MinIO.
//! This does NOT prove independence of the physical failure domain.
#![cfg(feature = "s3-covers")]

use aws_sdk_s3::{
    config::{BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{env, fs, io::Cursor, num::NonZeroU32, process::Command, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::{
    cover_review::PgCoverReview,
    cover_slots::{PgCoverSlots, PublicCoverKind},
    PgCatalog,
};
use tor_ingest::{
    repositories::{cover_jobs::PgCoverJobs, s3::S3CoverStore},
    services::{cover_jobs::CoverObjectStore, image_pipeline::RECIPE_VERSION},
};
use tor_reading::repositories::PgReader;
use tor_web::cover_worker::{CoverRunner, CoverRunnerMonitor};
use tower::ServiceExt;

const SOURCE_DB: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci";
const RESTORED_DB: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_offsite_restore_ci";
const ENDPOINT: &str = "http://127.0.0.1:9010";
const SOURCE: &str = "tor-fnd05-cover-source-ci";
const VAULT: &str = "tor-fnd05-cover-vault-ci";
const RESTORED: &str = "tor-fnd05-cover-restored-ci";
const SOURCE_KEY: &str = "staging/covers/sources/fnd05-recovery.png";
const MANIFEST_KEY: &str = "fnd05/active-cover.json";
const MAX: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Variant {
    kind: String,
    key: String,
    size: usize,
    sha: [u8; 32],
    revision: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    source_bucket: String,
    book: i64,
    job: i64,
    variants: Vec<Variant>,
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn isolated(db_env: &str, expected: &str) -> Client {
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert!(env::var(db_env).is_ok_and(|v| v == expected));
    assert_eq!(env::var("TOR_DR_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    assert_eq!(
        env::var("TOR_DR_S3_ACCESS_KEY").as_deref(),
        Ok("tor-dr-ci-user")
    );
    assert_eq!(
        env::var("TOR_DR_S3_SECRET_KEY").as_deref(),
        Ok("tor-dr-ci-secret-only")
    );
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                "tor-dr-ci-user",
                "tor-dr-ci-secret-only",
                None,
                None,
                "fnd05-cover-recovery",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .build(),
    )
}

async fn pg(url: &'static str, sql: String) -> String {
    tokio::task::spawn_blocking(move || {
        let output = Command::new("psql")
            .args([
                "-XAtq",
                "--no-password",
                "-v",
                "ON_ERROR_STOP=1",
                "-d",
                url,
                "-c",
                &sql,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "isolated cover fixture SQL failed");
        String::from_utf8(output.stdout).unwrap().trim().into()
    })
    .await
    .unwrap()
}

async fn inventory(slots: &PgCoverSlots, book: i64) -> Vec<Variant> {
    let mut result = Vec::new();
    for (kind, name) in [
        (PublicCoverKind::Card, "card"),
        (PublicCoverKind::Main, "main"),
    ] {
        let variant = slots.public_variant(book, kind).await.unwrap().unwrap();
        let size = usize::try_from(variant.size_bytes).unwrap();
        assert!((1..=MAX).contains(&size));
        assert!(variant.storage_key.starts_with("staging/covers/variants/"));
        result.push(Variant {
            kind: name.into(),
            key: variant.storage_key,
            size,
            sha: variant.sha256,
            revision: variant.revision,
        });
    }
    assert_ne!(result[0].key, result[1].key);
    result
}

async fn get_verified(store: &S3CoverStore, variant: &Variant) -> Vec<u8> {
    let bytes = store.get_bounded(&variant.key, variant.size).await.unwrap();
    assert_eq!(bytes.len(), variant.size);
    assert_eq!(sha(&bytes), variant.sha);
    assert_eq!(bytes.get(..4), Some(&b"RIFF"[..]));
    assert_eq!(bytes.get(8..12), Some(&b"WEBP"[..]));
    bytes
}

async fn put(client: &Client, bucket: &str, key: &str, mime: &str, bytes: Vec<u8>) {
    let digest = hex::encode(sha(&bytes));
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(mime)
        .metadata("sha256", digest)
        .if_none_match("*")
        .body(ByteStream::from(bytes))
        .send()
        .await
        .unwrap();
}

async fn get_http(app: &Router, path: &str) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    if status == StatusCode::OK && path.ends_with(".webp") {
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
        assert!(!response.headers().contains_key(header::LOCATION));
    }
    (
        status,
        to_bytes(response.into_body(), MAX).await.unwrap().to_vec(),
    )
}

#[tokio::test]
#[ignore = "seed only inside isolated sealed-remote-copy before pg_dump"]
async fn seed_active_cover_backup() {
    let client = isolated("TEST_DATABASE_URL", SOURCE_DB);
    for bucket in [SOURCE, VAULT] {
        client.create_bucket().bucket(bucket).send().await.unwrap();
    }
    let catalog = PgCatalog::connect(SOURCE_DB).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let hash = hash_password("FND05 disposable cover recovery fixture").unwrap();
    let owner_token = generate_session_token();
    let moderator_token = generate_session_token();
    let owner = accounts
        .register_with_session(
            "fnd05-cover-owner@example.invalid",
            "Recovery cover owner",
            &hash,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let moderator_digest = session_token_digest(&moderator_token);
    let moderator = accounts
        .register_with_session(
            "fnd05-cover-reviewer@example.invalid",
            "Recovery cover reviewer",
            &hash,
            &moderator_digest,
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        SOURCE_DB,
        format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({moderator},'moderator',true)"
        ),
    )
    .await;
    let book: i64 = pg(
        SOURCE_DB,
        format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:fnd05:active-cover','ru','FND05 recovered cover book','public',{owner},'basic') RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let store = S3CoverStore::new(client.clone(), SOURCE.into()).unwrap();
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_fn(120, 190, |x, y| {
        Rgb([41, x as u8, y as u8])
    }))
    .write_to(&mut png, ImageFormat::Png)
    .unwrap();
    let bytes = png.into_inner();
    let digest = sha(&bytes);
    store
        .put_source_verified(SOURCE_KEY, &bytes, "image/png", &digest)
        .await
        .unwrap();
    let jobs = PgCoverJobs::new(pool.clone());
    let source = jobs
        .register_verified_source(
            book,
            owner,
            SOURCE_KEY,
            "image/png",
            bytes.len() as i64,
            &digest,
        )
        .await
        .unwrap();
    let job = jobs
        .enqueue(source, book, owner, 0, RECIPE_VERSION)
        .await
        .unwrap();
    let monitor = CoverRunnerMonitor::default();
    let handle = CoverRunner::new(catalog.clone(), Arc::new(store.clone()), monitor.clone())
        .spawn(Duration::from_millis(40), Duration::from_secs(90));
    let done = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if monitor.snapshot().attached >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await;
    handle.abort();
    let _ = handle.await;
    done.expect("cover worker must produce two variants");
    let slots = PgCoverSlots::new(pool.clone());
    assert_eq!(slots.current_slot(book).await.unwrap(), None);
    let reviews = PgCoverReview::new(pool.clone());
    let review = reviews
        .pending(&moderator_digest, None)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.job_id == job)
        .unwrap();
    reviews
        .decide(&moderator_digest, review.id, 0, true, "")
        .await
        .unwrap();
    assert_eq!(slots.current_slot(book).await.unwrap(), Some((job, 1)));

    let variants = inventory(&slots, book).await;
    let vault = S3CoverStore::new(client.clone(), VAULT.into()).unwrap();
    for variant in &variants {
        let bytes = get_verified(&store, variant).await;
        put(&client, VAULT, &variant.key, "image/webp", bytes.clone()).await;
        assert_eq!(get_verified(&vault, variant).await, bytes);
    }
    let manifest = serde_json::to_vec(&Manifest {
        source_bucket: SOURCE.into(),
        book,
        job,
        variants,
    })
    .unwrap();
    // Independent digest survives the test process; the actual manifest and
    // WebP bytes are read back only from the vault on the restored side.
    let digest = hex::encode(sha(&manifest));
    fs::write(env::var("FND05_COVER_MANIFEST_SHA").unwrap(), digest).unwrap();
    put(&client, VAULT, MANIFEST_KEY, "application/json", manifest).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "verify only after isolated sealed-remote-copy pg_restore"]
async fn verify_restored_active_cover() {
    let client = isolated("FND05_RESTORED_DATABASE_URL", RESTORED_DB);
    let output = client
        .get_object()
        .bucket(VAULT)
        .key(MANIFEST_KEY)
        .send()
        .await
        .unwrap();
    let mut manifest_bytes = Vec::new();
    output
        .body
        .into_async_read()
        .take(8193)
        .read_to_end(&mut manifest_bytes)
        .await
        .unwrap();
    assert!(manifest_bytes.len() <= 8192);
    assert_eq!(
        hex::encode(sha(&manifest_bytes)),
        fs::read_to_string(env::var("FND05_COVER_MANIFEST_SHA").unwrap()).unwrap()
    );
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest.source_bucket, SOURCE);
    assert_eq!(manifest.variants.len(), 2);
    let catalog = PgCatalog::connect(RESTORED_DB).await.unwrap();
    let pool = catalog.pool();
    let slots = PgCoverSlots::new(pool.clone());
    assert_eq!(
        slots.current_slot(manifest.book).await.unwrap(),
        Some((manifest.job, 1))
    );
    assert_eq!(inventory(&slots, manifest.book).await, manifest.variants);

    // The restored app uses a fresh bucket. The source still exists but
    // must not be used as a fallback, and no image may be regenerated.
    client.create_bucket().bucket(RESTORED).send().await.unwrap();
    let restored = S3CoverStore::new(client.clone(), RESTORED.into()).unwrap();
    let vault = S3CoverStore::new(client.clone(), VAULT.into()).unwrap();
    let app = tor_web::router_with_postgres_reading_covers(
        catalog.clone(),
        PgReader::new(pool.clone()),
        Arc::new(restored.clone()),
    );
    for variant in &manifest.variants {
        let path = format!("/covers/{}/{}.webp", manifest.book, variant.kind);
        let (status, missing) = get_http(&app, &path).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(missing.is_empty());
        let bytes = get_verified(&vault, variant).await;
        put(&client, RESTORED, &variant.key, "image/webp", bytes.clone()).await;
        assert_eq!(get_verified(&restored, variant).await, bytes);
        let (status, returned) = get_http(&app, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(returned, bytes);
    }

    let (status, page) = get_http(&app, &format!("/books/{}", manifest.book)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&page).contains("FND05 recovered cover book"));
    pg(
        RESTORED_DB,
        format!("UPDATE books SET admin_hold=TRUE WHERE id={}", manifest.book),
    )
    .await;
    for variant in &manifest.variants {
        let path = format!("/covers/{}/{}.webp", manifest.book, variant.kind);
        assert_eq!(get_http(&app, &path).await.0, StatusCode::NOT_FOUND);
    }
    pool.close().await;
    println!("FND05_RESTORED_ACTIVE_COVER_HTTP_OK");
}
