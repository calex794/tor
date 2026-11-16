//! DROP-R4b1: real disposable PostgreSQL18 + private MinIO proof -> B attachment.
//! Requires the existing paid-private-e2e loopback services; never production.
#![cfg(feature = "s3-free")]
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{env, process::Command};
use tor_books::{
    repositories::{
        free_pack_attach::PgFreePackCatalog, free_s3_publish::PgFreeS3Publication, PgCatalog,
    },
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_contracts::drop_pack::{PackPlacementFrame, PackPlacementReceipt, PackPlacementTarget};
use tor_storage::{
    repositories::{free_chapter_s3::S3FreeChapterStore, free_pack_proof_s3::S3PackProofStore},
    services::{
        free_chapter::{upload_and_verify, FreeChapterObject},
        free_pack_write::prepare_free_pack,
    },
    repositories::free_pack_write_s3::S3FreePackWriter,
};
use tor_web::free_pack_worker::{finalize_one_verified_pack, upload_one_approved_pack};

const BUCKET: &str = "tor-drop-r4b1-private-ci";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let out = Command::new("psql")
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
        .unwrap();
    assert!(
        out.status.success(),
        "PG fixture failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn s3() -> Client {
    let endpoint = env::var("TOR_TEST_S3_ENDPOINT").unwrap();
    assert_eq!(endpoint, "http://127.0.0.1:9000");
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "drop-r4b1-test",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build(),
    )
}

fn sha(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Creates a standards-compatible Zstd seekable PACK, not an ad hoc bundle.
/// The trusted descriptor must be independently revalidated after S3 upload.
fn build_pack(
    book: i64,
    edition_bodies: &[(i64, i64, String)],
    variant: &str,
) -> (PackPlacementReceipt, Vec<u8>, Vec<u8>) {
    const SKIPPABLE_MAGIC: u32 = 0x184D2A5E;
    const SEEKABLE_MAGIC: u32 = 0x8F92EAB1;
    let mut pack = Vec::new();
    let mut table = Vec::new();
    let mut entries = Vec::new();
    let mut frames = Vec::new();
    for (index, (chapter, version, body)) in edition_bodies.iter().enumerate() {
        let raw = body.as_bytes();
        let compressed = zstd::bulk::compress(raw, 3).unwrap();
        let offset = pack.len() as u64;
        table.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        table.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        entries.push(json!({
            "edition_id": format!("{chapter}:{version}"),
            "frame": index, "offset": offset,
            "encoded_bytes": compressed.len(), "raw_bytes": raw.len(),
            "sha256": hex::encode(sha(raw))
        }));
        frames.push(PackPlacementFrame {
            chapter_id: *chapter,
            version_id: *version,
            content_sha256: sha(raw),
            size_bytes: raw.len() as u64,
            frame_offset: offset,
            frame_bytes: compressed.len() as u64,
        });
        pack.extend_from_slice(&compressed);
    }
    let seek_table_start = pack.len() as u64;
    pack.extend_from_slice(&SKIPPABLE_MAGIC.to_le_bytes());
    pack.extend_from_slice(&(table.len() as u32 + 9).to_le_bytes());
    pack.extend_from_slice(&table);
    pack.extend_from_slice(&(edition_bodies.len() as u32).to_le_bytes());
    pack.push(0);
    pack.extend_from_slice(&SEEKABLE_MAGIC.to_le_bytes());
    let manifest = serde_json::to_vec(&json!({
        "format": "tor-store-public-seekable-zstd",
        "manifest_version": 1,
        "zstd_seekable_version": "0.1.0",
        "pack_sha256": hex::encode(sha(&pack)),
        "pack_encoded_bytes": pack.len(),
        "seek_table_offset": seek_table_start,
        "seek_table_bytes": pack.len() as u64 - seek_table_start,
        "frames": edition_bodies.len(),
        "entries": entries,
    }))
    .unwrap();
    let key = format!("private/free-packs/ci/drop-r4b1-{variant}-{book}.zst");
    (
        PackPlacementReceipt {
            target: PackPlacementTarget {
                bucket: BUCKET.to_owned(),
                pack_key: key.clone(),
                manifest_key: key.replace(".zst", ".manifest.json"),
                pack_sha256: sha(&pack),
                manifest_sha256: sha(&manifest),
                pack_bytes: pack.len() as u64,
                manifest_bytes: manifest.len() as u64,
                seek_table_start,
            },
            frames,
        },
        pack,
        manifest,
    )
}

async fn upload(client: &Client, receipt: &PackPlacementReceipt, pack: Vec<u8>, manifest: Vec<u8>) {
    client
        .put_object()
        .bucket(BUCKET)
        .key(&receipt.target.pack_key)
        .if_none_match("*")
        .body(ByteStream::from(pack))
        .send()
        .await
        .unwrap();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&receipt.target.manifest_key)
        .if_none_match("*")
        .body(ByteStream::from(manifest))
        .send()
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PG18 and private loopback MinIO"]
async fn drop_r4b1_real_readback_lease_and_atomic_candidate_attach() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    let client = s3();
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    let pool = PgCatalog::connect(&url).await.unwrap().pool().clone();
    let actor: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
        VALUES('drop-r4b1-owner@ci.example','DROP worker','$argon2id$test') RETURNING id",
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:drop:r4b1','ru','DROP R4b1','public',{actor}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let publisher = PgFreeS3Publication::new(pool.clone());
    let single = S3FreeChapterStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let texts = ["First source 😀", "Second independent source 中文"];
    let mut editions = Vec::new();
    for (index, body) in texts.iter().enumerate() {
        let key = (b'w' + index as u8) as char;
        let cmd = key.to_string().repeat(43);
        let request = save_chapter(SaveChapterRequest {
            book_id: book as u64,
            chapter_id: None,
            expected_revision: 0,
            number: (index + 1) as i32,
            title: "DROP fixture",
            body,
            access: "free",
            price_amount: 0,
            command_key: &cmd,
        })
        .unwrap();
        let pending = publisher.prepare_new(actor, &request).await.unwrap();
        let obj =
            FreeChapterObject::for_markdown(pending.chapter_id, pending.version_id, body).unwrap();
        upload_and_verify(&single, &obj, body).await.unwrap();
        publisher
            .attach_verified_and_publish(
                actor,
                book,
                (pending.chapter_id, pending.version_id),
                (BUCKET, *obj.sha256(), obj.size_bytes()),
            )
            .await
            .unwrap();
        editions.push((pending.chapter_id, pending.version_id, (*body).to_owned()));
    }

    let catalog = PgFreePackCatalog::new(pool.clone());
    let sources: Vec<_> = editions
        .iter()
        .map(|(chapter, version, body)| {
            (FreeChapterObject::for_markdown(*chapter, *version, body).unwrap(),
             body.as_bytes().to_vec())
        })
        .collect();
    let prepared = prepare_free_pack(
        BUCKET,
        &format!("private/free-packs/ci/drop-r4b2-approved-{book}.zst"),
        &sources,
    )
    .unwrap();
    let receipt = prepared.receipt().clone();
    let job = catalog
        .schedule_pack_job(actor, book, &receipt)
        .await
        .unwrap();
    assert_eq!(
        catalog
            .schedule_pack_job(actor, book, &receipt)
            .await
            .unwrap(),
        job
    );
    let claim = catalog.claim_pack_job(120).await.unwrap().unwrap();
    assert_eq!(claim.job_id, job);
    assert_eq!(claim.epoch, 1);
    assert!(catalog.renew_pack_job(job, claim.epoch, 120).await.unwrap());
    let proof = S3PackProofStore::new(client.clone());
    let writer = S3FreePackWriter::new(client.clone());
    let pack_id = upload_one_approved_pack(&catalog, &writer, &claim, &prepared)
        .await
        .unwrap();
    assert!(pack_id > 0);
    assert_eq!(
        finalize_one_verified_pack(&catalog, &proof, &claim)
            .await
            .unwrap(),
        pack_id,
        "lost PG COMMIT ACK must not duplicate generations"
    );
    let candidates: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM free_edition_storage_generations WHERE pack_id={pack_id}"),
    )
    .parse()
    .unwrap();
    assert_eq!(candidates, 2);
    let active: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage
         WHERE (chapter_id,version_id) IN (({},{}),({},{}))",
            editions[0].0, editions[0].1, editions[1].0, editions[1].1
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(active, 0, "R4b1 never changes live reader pointers");
    assert!(!catalog.renew_pack_job(job, claim.epoch, 90).await.unwrap());
    assert!(catalog.claim_pack_job(90).await.unwrap().is_none());

    // A corrupt or missing object must never enter the physical PG catalog.
    let (bad, pack, manifest) = build_pack(book, &editions, "tampered");
    upload(&client, &bad, pack, manifest).await;
    let bad_job = catalog.schedule_pack_job(actor, book, &bad).await.unwrap();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&bad.target.pack_key)
        .body(ByteStream::from(b"not the approved pack".to_vec()))
        .send()
        .await
        .unwrap();
    let bad_claim = catalog.claim_pack_job(90).await.unwrap().unwrap();
    assert_eq!(bad_claim.job_id, bad_job);
    assert!(matches!(
        finalize_one_verified_pack(&catalog, &proof, &bad_claim).await,
        Err(AuthorError::Unavailable)
    ));
    let bad_count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM free_storage_packs WHERE object_key='{}'",
            bad.target.pack_key
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(bad_count, 0);

    // Cancellation revokes a prior claim, independently of successful S3 proof.
    let (cancelled, pack, manifest) = build_pack(book, &editions, "cancelled");
    upload(&client, &cancelled, pack, manifest).await;
    let cancel_job = catalog
        .schedule_pack_job(actor, book, &cancelled)
        .await
        .unwrap();
    // Earlier corrupt job has a still-live lease, so claim selects this pending one.
    let cancel_claim = catalog.claim_pack_job(90).await.unwrap().unwrap();
    assert_eq!(cancel_claim.job_id, cancel_job);
    pg(
        &url,
        &format!(
            "UPDATE free_pack_jobs SET status='cancelled',locked_until=NULL WHERE id={cancel_job}"
        ),
    );
    assert!(matches!(
        finalize_one_verified_pack(&catalog, &proof, &cancel_claim).await,
        Err(AuthorError::Conflict)
    ));
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated MinIO S3 on loopback"]
async fn drop_r4b2_real_s3_lost_ack_retry_and_wrong_existing_object_fail_closed() {
    let client = s3();
    // Shared test bucket may already exist from the R4b1 PG+MinIO test.
    let _ = client.create_bucket().bucket(BUCKET).send().await;
    let writer = S3FreePackWriter::new(client.clone());
    let original = "Repack source exact bytes, 中文😀";
    let edition = FreeChapterObject::for_markdown(921_000, 922_000, original).unwrap();
    let sources = vec![(edition, original.as_bytes().to_vec())];
    let prepared = prepare_free_pack(
        BUCKET,
        "private/free-packs/ci/drop-r4b2-lost-ack.zst",
        &sources,
    )
    .unwrap();

    // Simulate a committed initial PACK PUT for which the client lost its ACK.
    // The writer's next PUT gets a 412, but a full verified GET recovers safely.
    client.put_object()
        .bucket(BUCKET).key(&prepared.target().pack_key)
        .if_none_match("*")
        .body(ByteStream::from(prepared.pack_bytes().to_vec()))
        .send().await.unwrap();
    let proved = writer.upload_and_verify(&prepared).await.unwrap();
    assert_eq!(proved.placement_receipt(), *prepared.receipt());
    assert_eq!(writer.upload_and_verify(&prepared).await.unwrap().placement_receipt(),
               *prepared.receipt(), "second retry must verify exact existing bytes");

    let other = prepare_free_pack(
        BUCKET,
        "private/free-packs/ci/drop-r4b2-wrong-object.zst",
        &sources,
    )
    .unwrap();
    client.put_object()
        .bucket(BUCKET).key(&other.target().pack_key)
        .if_none_match("*")
        .body(ByteStream::from(b"forged existing private PACK".to_vec()))
        .send().await.unwrap();
    assert_eq!(writer.upload_and_verify(&other).await.err(),
               Some(tor_storage::domain::provider::StorageError::Corrupt));
    let untouched = client.get_object().bucket(BUCKET)
        .key(&other.target().pack_key).send().await.unwrap()
        .body.collect().await.unwrap().into_bytes();
    assert_eq!(untouched.as_ref(), b"forged existing private PACK",
               "writer may never overwrite an object occupying an approved key");
}
