//! H-web-10: real S3 requests against an isolated, disposable MinIO process.
//! No production bucket, credentials, internet provider, PostgreSQL or user books.
//! Only runs via an explicit ignored test with a loopback-only endpoint.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use tor_ingest::{
    domain::cover::CoverError, repositories::s3::S3CoverStore,
    services::cover_jobs::CoverObjectStore,
};

const BUCKET: &str = "tor-private-cover-ci";
const CARD_KEY: &str = "staging/covers/variants/provider-card.webp";
const MISSING_KEY: &str = "staging/covers/variants/provider-missing.webp";

fn webp() -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 60, Rgb([17, 29, 43])));
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::WebP).unwrap();
    encoded.into_inner()
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[tokio::test]
#[ignore = "requires an isolated loopback MinIO launched by tor-cover-s3.yml"]
async fn h_web_10_s3_provider_put_head_get_and_private_corruption() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("explicit local MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "provider test must not contact a remote S3 service"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test-only credential");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test-only credential");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(access, secret, None, None, "local-test"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create disposable private bucket");
    let store = S3CoverStore::new(client.clone(), BUCKET.into()).unwrap();

    let bytes = webp();
    assert!(bytes.starts_with(b"RIFF"));
    assert_eq!(bytes.get(8..12), Some(&b"WEBP"[..]));
    let sha = digest(&bytes);
    store
        .put_verified(CARD_KEY, &bytes)
        .await
        .expect("signed PUT plus verified S3 HEAD");
    assert!(store
        .exists_verified(CARD_KEY, &sha, bytes.len() as i64)
        .await
        .unwrap());
    assert!(!store
        .exists_verified(CARD_KEY, &[0_u8; 32], bytes.len() as i64)
        .await
        .unwrap());
    assert!(!store
        .exists_verified(CARD_KEY, &sha, bytes.len() as i64 + 1)
        .await
        .unwrap());
    assert_eq!(
        store.get_bounded(CARD_KEY, bytes.len()).await.unwrap(),
        bytes
    );
    assert_eq!(
        store.get_bounded(CARD_KEY, bytes.len() - 1).await,
        Err(CoverError::Invalid),
        "over-limit GET must not return partial object data"
    );
    assert_eq!(
        store.get_bounded(MISSING_KEY, 1024).await,
        Err(CoverError::Unavailable)
    );
    assert_eq!(
        store.get_bounded("../private", 1024).await,
        Err(CoverError::Invalid)
    );
    assert_eq!(
        store.get_bounded(CARD_KEY, 0).await,
        Err(CoverError::Invalid)
    );
    assert_eq!(
        store
            .put_verified("staging/covers/../escape.webp", &bytes)
            .await,
        Err(CoverError::Invalid)
    );
    assert_eq!(
        store
            .put_verified("staging/covers/unverified-source.webp", &bytes)
            .await,
        Err(CoverError::Invalid),
        "worker must never overwrite source object"
    );
    assert_eq!(
        store
            .put_verified(CARD_KEY, &vec![0_u8; 8 * 1024 * 1024 + 1])
            .await,
        Err(CoverError::Invalid)
    );

    // HEAD metadata is not proof of object bytes. Tamper using the test owner
    // with a matching metadata claim: the H HTTP adapter must still SHA-check GET.
    let mut damaged = bytes.clone();
    let last = damaged.last_mut().unwrap();
    *last ^= 0x7f;
    let claimed_sha: String = sha.iter().map(|b| format!("{b:02x}")).collect();
    client
        .put_object()
        .bucket(BUCKET)
        .key(CARD_KEY)
        .content_type("image/webp")
        .metadata("sha256", claimed_sha)
        .body(ByteStream::from(damaged.clone()))
        .send()
        .await
        .unwrap();
    assert!(
        store
            .exists_verified(CARD_KEY, &sha, bytes.len() as i64)
            .await
            .unwrap(),
        "HEAD can be forged by a writer: HTTP layer must verify the bytes"
    );
    let fetched = store.get_bounded(CARD_KEY, bytes.len()).await.unwrap();
    assert_eq!(fetched, damaged);
    assert_ne!(digest(&fetched), sha, "HTTP verification must reject this");

    let rebuilt = S3CoverStore::new(client, BUCKET.into()).unwrap();
    assert_eq!(
        rebuilt.put_verified(CARD_KEY, &bytes).await,
        Ok(()),
        "correct retry restores verified object data"
    );
    assert_eq!(
        rebuilt.get_bounded(CARD_KEY, bytes.len()).await.unwrap(),
        bytes
    );
}
