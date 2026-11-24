//! #533: opt-in MinIO version experiment; not the required G-09 gate.
#![cfg(feature = "s3")]

#[path = "s3_private_provider/get_diagnostics.rs"]
mod get_diagnostics;

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use get_diagnostics::{first_get, CloseConnection, ObserveFirstGet};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, EncryptedObjectKind, ObjectId, ObjectRef, PositiveId,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::s3::S3PrivateStore,
    services::{
        crypto::{seal_chapter, sha256_bytes, ContentKek},
        provider::PrivateObjectStore,
    },
};

const ROUNDS: u16 = 10000;
const ENDPOINT: &str = "http://127.0.0.1:9000";

fn client(close: bool) -> Client {
    assert_eq!(
        std::env::var("TOR_TEST_S3_ENDPOINT").as_deref(),
        Ok(ENDPOINT),
        "dedicated loopback fixture only"
    );
    let mut config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "tor-g09-ab-ci",
            "tor-g09-ab-synthetic-ci-only",
            None,
            None,
            "g09-ab-loopback",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .interceptor(ObserveFirstGet);
    if close {
        config = config.interceptor(CloseConnection);
    }
    Client::from_conf(config.build())
}

#[tokio::test]
#[ignore = "dedicated disposable MinIO version comparison only"]
async fn g09_minio_ab() {
    for close in [false, true] {
        sample(close).await;
    }
}

async fn sample(close: bool) {
    let mode = if close { "close" } else { "pooled" };
    let bucket = format!("tor-g09-ab-{mode}");
    let sdk = client(close);
    sdk.create_bucket()
        .bucket(&bucket)
        .send()
        .await
        .expect("create disposable bucket");
    let store = S3PrivateStore::new(sdk, bucket.clone()).unwrap();
    let independent = S3PrivateStore::new(client(true), bucket).unwrap();
    let version = ChapterVersionRef {
        chapter_id: PositiveId::new(7781).unwrap(),
        version_id: PositiveId::new(7791).unwrap(),
    };
    let kek = ContentKek::from_bytes([53; 32]);
    let mut unavailable = 0;
    for round in 1..=ROUNDS {
        let id = ObjectId::parse(&format!("c0f20000-0000-4000-8000-{round:012x}")).unwrap();
        let original = seal_chapter(
            version,
            id.clone(),
            &format!("original synthetic ciphertext {round}"),
            &kek,
        )
        .unwrap();
        let competing = seal_chapter(
            version,
            id.clone(),
            &format!("competing synthetic ciphertext {round}"),
            &kek,
        )
        .unwrap();
        let record = ObjectRef::new(
            id.clone(),
            EncryptedObjectKind::Chapter,
            sha256_bytes(&original.ciphertext),
            original.ciphertext.len() as u64,
        )
        .unwrap();
        let forged = ObjectRef::new(
            id,
            EncryptedObjectKind::Chapter,
            sha256_bytes(&competing.ciphertext),
            competing.ciphertext.len() as u64,
        )
        .unwrap();
        assert_ne!(record.sha256_ciphertext, forged.sha256_ciphertext);
        assert_eq!(
            store.put_immutable(&record, &original.ciphertext).await,
            Ok(()),
            "initial PUT failed: {mode} round {round}"
        );
        assert_eq!(
            store.put_immutable(&forged, &competing.ciphertext).await,
            Err(StorageError::Conflict),
            "conflicting PUT was not rejected: {mode} round {round}"
        );
        // Exactly the strict G-09 sequence. No intervening HEAD, delay or retry.
        match first_get(&store, &record).await {
            Ok(bytes) => assert!(
                bytes == original.ciphertext,
                "first GET ciphertext mismatch: {mode} round {round}"
            ),
            Err(StorageError::Unavailable) => {
                unavailable += 1;
                eprintln!("G09_AB_FIRST_UNAVAILABLE mode={mode} round={round}");
                // Failure is counted permanently. Independent verification is
                // evidence of intact data, never a substitute for first GET.
                assert_eq!(independent.head_verified(&record).await, Ok(true));
                let bytes = independent.get_verified(&record).await.unwrap();
                assert!(
                    bytes == original.ciphertext,
                    "independent ciphertext mismatch: {mode} round {round}"
                );
            }
            Err(error) => panic!("unexpected first GET {error:?}: {mode} round {round}"),
        }
    }
    println!(
        "\nG09_AB_RESULT mode={mode} attempted={ROUNDS} first_get_ok={} first_get_unavailable={unavailable} integrity_verified={ROUNDS}",
        ROUNDS - unavailable
    );
}
