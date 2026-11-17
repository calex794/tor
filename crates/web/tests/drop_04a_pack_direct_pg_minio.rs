//! DROP-04a: real private MinIO + PG18 direct PACK staged receipt.
//! Source goes directly from the approved import text to one seekable PACK;
//! NO free_chapter_storage_objects, publication, reader or per-chapter PUT.
#![cfg(feature = "s3-free")]
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use sha2::{Digest, Sha256};
use std::{env, process::Command};
use tor_books::{
    repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog},
    services::{
        authoring::AuthorError,
        chapter_file_import::{fingerprint, parse_text},
    },
};
use tor_imports::{domain::ChapterImportClaim, repositories::postgres::PgImportJobs};
use tor_storage::{
    repositories::{
        free_chapter_s3::S3FreeChapterStore, free_pack_proof_s3::S3PackProofStore,
        free_pack_write_s3::S3FreePackWriter,
    },
    services::free_chapter::FreeChapterObject,
};
use tor_web::free_author_publish::PgFreeAuthorPublisher;

const BUCKET: &str = "tor-drop-direct-stage-ci";
const ENDPOINT: &str = "http://127.0.0.1:9000";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
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
        .unwrap();
    assert!(
        output.status.success(),
        "PG fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn s3() -> Client {
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "drop-04a-test",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL18 + MinIO, never production"]
async fn drop_04a_stages_direct_pack_without_any_single_and_fences_stale_epoch() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    let client = s3();
    let _ = client.create_bucket().bucket(BUCKET).send().await;
    let pool = PgCatalog::connect(&url).await.unwrap().pool().clone();
    let actor: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('drop-04a-ci@tor.example','DROP direct CI','$argon2id$test')
         RETURNING id",
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:drop:04a','ru','DROP direct','draft',{actor})
         RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let source = "# Глава 1: Начало\nОдин текст 😀\n\n# Глава 2: Далее\nВторой текст 中文";
    let chapters = parse_text("direct-chapters.txt", source).unwrap();
    assert_eq!(chapters.len(), 2);
    let key = "d".repeat(43);
    let input_key = format!("staging/chapters/{actor}/{book}/{key}");
    let hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&input_key)
        .body(ByteStream::from(source.as_bytes().to_vec()))
        .send()
        .await
        .unwrap();
    let read = client
        .get_object()
        .bucket(BUCKET)
        .key(&input_key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(Sha256::digest(&read).as_slice(), hash.as_slice());
    let digest = fingerprint(book, &chapters);
    let jobs = PgImportJobs::new(pool.clone());
    let job = jobs
        .enqueue_verified(
            actor,
            book,
            &key,
            &input_key,
            "direct-chapters.txt",
            &hash,
            source.len() as i64,
            &digest,
            chapters.len() as i32,
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "UPDATE chapter_import_jobs
         SET status='running',attempts=1,lease_epoch=1,
             locked_until=clock_timestamp()+interval '2 minutes'
         WHERE id={job}"
        ),
    );
    let claim = ChapterImportClaim {
        job_id: job,
        parent_trace_id: None,
        book_id: book,
        actor_id: actor,
        command_key: key,
        source_key: input_key,
        source_filename: "direct-chapters.txt".into(),
        source_sha256: hash.to_vec(),
        source_bytes: source.len() as i64,
        approved_fingerprint: digest.to_vec(),
        expected_chapter_count: chapters.len() as i32,
        attempts: 1,
        epoch: 1,
    };
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.into()).unwrap();
    let author =
        PgFreeAuthorPublisher::new(PgFreeS3Publication::new(pool.clone()), store, BUCKET.into())
            .unwrap();
    let writer = S3FreePackWriter::new(client.clone());
    let first = author
        .stage_direct_pack_only(&claim, &chapters, &writer)
        .await
        .unwrap();
    assert_eq!(first.frames.len(), 2);
    let second = author
        .stage_direct_pack_only(&claim, &chapters, &writer)
        .await
        .unwrap();
    assert_eq!(
        first, second,
        "lost commit ACK cannot rewrite the immutable PACK"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM free_pack_direct_import_stages WHERE job_id={job}")
        ),
        "1"
    );
    let candidates: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM free_chapter_storage_objects f
          JOIN free_chapter_import_reservations r ON r.version_id=f.version_id
          WHERE r.book_id={book}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(candidates, 0, "no per-edition SINGLE upload was performed");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id={book}"
            )
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapters WHERE book_id={book}")
        ),
        "0",
        "all identities remain reserved and invisible"
    );
    let expected: Vec<_> = first
        .frames
        .iter()
        .map(|f| {
            FreeChapterObject::from_metadata(
                f.chapter_id,
                f.version_id,
                f.content_sha256,
                f.size_bytes,
            )
            .unwrap()
        })
        .collect();
    let verified = S3PackProofStore::new(client.clone())
        .verify_independent_full_get(
            &tor_storage::services::free_pack_proof::PackProofTarget {
                bucket: first.target.bucket.clone(),
                pack_key: first.target.pack_key.clone(),
                manifest_key: first.target.manifest_key.clone(),
                pack_sha256: first.target.pack_sha256,
                manifest_sha256: first.target.manifest_sha256,
                pack_bytes: first.target.pack_bytes,
                manifest_bytes: first.target.manifest_bytes,
                seek_table_start: first.target.seek_table_start,
            },
            &expected,
        )
        .await
        .unwrap();
    assert_eq!(verified.placement_receipt(), first);
    // Exact job source fingerprint is B authority, not a client descriptor.
    let mut forged = claim.clone();
    forged.approved_fingerprint[0] ^= 1;
    assert!(matches!(
        author
            .stage_direct_pack_only(&forged, &chapters, &writer)
            .await,
        Err(AuthorError::Conflict)
    ));
    // Late worker with an expired/stolen epoch cannot stage or publish anything.
    pg(
        &url,
        &format!(
            "UPDATE chapter_import_jobs SET lease_epoch=lease_epoch+1,
         locked_until=clock_timestamp()+interval '2 minutes' WHERE id={job}"
        ),
    );
    assert!(matches!(
        author
            .stage_direct_pack_only(&claim, &chapters, &writer)
            .await,
        Err(AuthorError::Conflict)
    ));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id={book}"
            )
        ),
        "0"
    );
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL18 + private MinIO; no live PACK HTTP"]
async fn drop_04b_direct_pack_finalizer_is_atomic_replayable_and_stays_private() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    let client = s3();
    let _ = client.create_bucket().bucket(BUCKET).send().await;
    let pool = PgCatalog::connect(&url).await.unwrap().pool().clone();
    let actor: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('drop-04b-ci@tor.example','DROP finalizer CI','$argon2id$test')
         RETURNING id",
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:drop:04b','ru','DROP direct finalize','draft',{actor})
         RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let source =
        "# Глава 1: Первая\nНеизменяемый текст 😀\n\n# Глава 2: Вторая\nВторой диапазон 中文";
    let chapters = parse_text("direct-finalize.txt", source).unwrap();
    assert_eq!(chapters.len(), 2);
    let key = "e".repeat(43);
    let input_key = format!("staging/chapters/{actor}/{book}/{key}");
    let source_hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&input_key)
        .body(ByteStream::from(source.as_bytes().to_vec()))
        .send()
        .await
        .unwrap();
    let source_fingerprint = fingerprint(book, &chapters);
    let jobs = PgImportJobs::new(pool.clone());
    let job_id = jobs
        .enqueue_verified(
            actor,
            book,
            &key,
            &input_key,
            "direct-finalize.txt",
            &source_hash,
            source.len() as i64,
            &source_fingerprint,
            2,
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "UPDATE chapter_import_jobs SET status='running',attempts=1,lease_epoch=1,
           locked_until=clock_timestamp()+interval '2 minutes' WHERE id={job_id}"
        ),
    );
    let claim = ChapterImportClaim {
        job_id,
        parent_trace_id: None,
        book_id: book,
        actor_id: actor,
        command_key: key,
        source_key: input_key,
        source_filename: "direct-finalize.txt".into(),
        source_sha256: source_hash.to_vec(),
        source_bytes: source.len() as i64,
        approved_fingerprint: source_fingerprint.to_vec(),
        expected_chapter_count: 2,
        attempts: 1,
        epoch: 1,
    };
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.into()).unwrap();
    let author =
        PgFreeAuthorPublisher::new(PgFreeS3Publication::new(pool.clone()), store, BUCKET.into())
            .unwrap();
    let writer = S3FreePackWriter::new(client.clone());
    let receipt = author
        .stage_direct_pack_only(&claim, &chapters, &writer)
        .await
        .unwrap();
    let proof = S3PackProofStore::new(client.clone());
    let assert_no_publication = || {
        assert_eq!(
            pg(
                &url,
                &format!("SELECT count(*) FROM chapters WHERE book_id={book}")
            ),
            "0"
        );
        assert_eq!(
            pg(
                &url,
                &format!(
                    "SELECT count(*) FROM free_storage_packs
             WHERE object_key='{}'",
                    receipt.target.pack_key
                )
            ),
            "0"
        );
        assert_eq!(
            pg(
                &url,
                &format!("SELECT status FROM chapter_import_jobs WHERE id={job_id}")
            ),
            "running"
        );
    };

    // An old, already superseded lease must not materialize any of the
    // reserved editions even when the staged S3 PACK is byte-for-byte valid.
    let mut stale = claim.clone();
    stale.epoch = 2;
    assert!(matches!(
        author
            .finalize_direct_pack_after_reproof(&stale, &chapters, &receipt, &proof)
            .await,
        Err(AuthorError::Conflict)
    ));
    assert_no_publication();

    // A moderation hold must abort the whole transaction. The source and
    // its staged proof remain available for a later authorized retry.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert!(matches!(
        author
            .finalize_direct_pack_after_reproof(&claim, &chapters, &receipt, &proof)
            .await,
        Err(AuthorError::Conflict)
    ));
    assert_no_publication();
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );

    assert_eq!(
        author
            .finalize_direct_pack_after_reproof(&claim, &chapters, &receipt, &proof)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT status||':'||saved_count FROM chapter_import_jobs WHERE id={job_id}")
        ),
        "succeeded:2"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
         JOIN chapter_version_content_shapes sh ON sh.version_id=v.id
         JOIN free_edition_active_storage a ON a.chapter_id=c.id
             AND a.version_id=v.id AND a.generation=1
         JOIN free_edition_storage_generations g
             ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
            AND g.generation=a.generation AND g.placement_kind='pack'
         JOIN free_storage_packs pk ON pk.id=g.pack_id
         WHERE c.book_id={book} AND pk.object_key='{}'",
                receipt.target.pack_key
            )
        ),
        "2"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects f
         JOIN chapter_versions v ON v.id=f.version_id
         JOIN chapters c ON c.id=v.chapter_id WHERE c.book_id={book}"
            )
        ),
        "0",
        "PACK-direct must never write individual SINGLEs"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM free_storage_packs WHERE object_key='{}'",
                receipt.target.pack_key
            )
        ),
        "1"
    );

    // A lost acknowledgement from the successful PG COMMIT replays without
    // duplicating a pack, generation, chapter or import batch.
    assert_eq!(
        author
            .finalize_direct_pack_after_reproof(&claim, &chapters, &receipt, &proof)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapters WHERE book_id={book}")
        ),
        "2"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapter_import_batches WHERE book_id={book}")
        ),
        "1"
    );

    let mut altered_receipt = receipt.clone();
    altered_receipt.frames[0].content_sha256[0] ^= 1;
    assert!(
        author
            .finalize_direct_pack_after_reproof(&claim, &chapters, &altered_receipt, &proof)
            .await
            .is_err(),
        "even after success a forged receipt is never an ACK"
    );

    // The entire book remains private. No free SINGLE grant can reveal PACK.
    let c = tor_reading::repositories::free_s3_ref::PgFreeS3Ref::new(pool.clone());
    assert!(c
        .for_session(receipt.frames[0].chapter_id, None)
        .await
        .unwrap()
        .is_none());

    // The strengthened S349 trigger still rejects an unrelated bodyless
    // free publication with NO verified placement at all.
    let stranger: i64 = pg(&url, &format!(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES({book},999,'No verified bytes','draft','free',0,1) RETURNING id"
    )).parse().unwrap();
    let missing: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
         VALUES({stranger},1,decode(repeat('a',64),'hex')) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let invalid = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &url,
            "--command",
            &format!(
                "INSERT INTO chapter_publications(chapter_id,version_id)
                 VALUES({stranger},{missing})"
            ),
        ])
        .output()
        .unwrap();
    assert!(
        !invalid.status.success()
            && String::from_utf8_lossy(&invalid.stderr).contains("S349/DROP:"),
        "the S349/DROP guard must reject bodyless free editions without proof"
    );

    // DROP-06a: only the independently mounted HTTP router is under test.
    // A PACK-direct edition has no SINGLE grant; C authorizes the exact
    // active generation after the book is explicitly made public/basic.
    pg(
        &url,
        &format!("UPDATE books SET visibility='public',access_level='basic' WHERE id={book}"),
    );
    let c = tor_reading::repositories::free_s3_ref::PgFreeS3Ref::new(pool.clone());
    for frame in &receipt.frames {
        let authorized = c
            .for_active_pack_session(frame.chapter_id, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(authorized.version_id, frame.version_id);
        assert_eq!(authorized.sha256, frame.content_sha256);
        assert_eq!(authorized.bucket, BUCKET);
        assert!(
            c.for_session(frame.chapter_id, None)
                .await
                .unwrap()
                .is_none(),
            "PACK-direct has no SINGLE to presign"
        );
    }
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let app = tor_web::free_pack_http::router(
        tor_accounts::repositories::PgAccounts::from_pool(pool.clone()),
        false,
        "http://127.0.0.1:3098".into(),
        client.clone(),
        BUCKET.into(),
    )
    .unwrap();

    for (index, frame) in receipt.frames.iter().enumerate() {
        // DROP-06b: the browser receives a verified plan, not a whole-PACK URL.
        let plan = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/chapters/{}/free-source-plan",
                        frame.chapter_id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(plan.status(), StatusCode::OK);
        let body = to_bytes(plan.into_body(), 4096).await.unwrap();
        let plan: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(plan["schema"], "tor.free-source-plan.v1");
        assert_eq!(plan["transport"], "gateway");
        assert_eq!(plan["version_id"], frame.version_id.to_string());
        assert_eq!(plan["size_bytes"], chapters[index].body.len() as u64);
        assert!(plan.get("url").is_none(), "never disclose S3 capability");
        let uri = format!("/api/v1/chapters/{}/free-pack-source", frame.chapter_id);
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            response.headers()["x-tor-free-version-id"],
            frame.version_id.to_string()
        );
        let bytes = to_bytes(response.into_body(), 200_001).await.unwrap();
        assert_eq!(
            bytes.as_ref(),
            chapters[index].body.as_bytes(),
            "only exact requested Unicode chapter bytes; never the neighbor"
        );
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(actual, frame.content_sha256);
        // A browser with scripts disabled must receive the same exact frame
        // through one same-origin link, never the complete PACK.
        let plain = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/chapters/{}/free-source", frame.chapter_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(plain.status(), StatusCode::OK);
        assert_eq!(plain.headers()["content-type"], "text/plain; charset=utf-8");
        assert_eq!(plain.headers()["cache-control"], "no-store");
        let bytes = to_bytes(plain.into_body(), 200_001).await.unwrap();
        assert_eq!(bytes.as_ref(), chapters[index].body.as_bytes());
    }

    let uri = format!(
        "/api/v1/chapters/{}/free-pack-source",
        receipt.frames[0].chapter_id
    );
    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{uri}?mode=saved&version_id={}",
                    receipt.frames[0].version_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        saved.status(),
        StatusCode::NOT_FOUND,
        "a client-selected V1 without saved pin cannot grant any packed frame"
    );
    let denied_plan = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/chapters/{}/free-source-plan?mode=saved&version_id={}",
                    receipt.frames[0].chapter_id, receipt.frames[0].version_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_plan.status(), StatusCode::NOT_FOUND);
    let denied_plain = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/chapters/{}/free-source?mode=saved&version_id={}",
                    receipt.frames[0].chapter_id, receipt.frames[0].version_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_plain.status(), StatusCode::NOT_FOUND);

    let arbitrary_range = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("Range", "bytes=0-9999999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        arbitrary_range.status(),
        StatusCode::BAD_REQUEST,
        "the client cannot obtain any arbitrary adjacent frame or entire PACK"
    );

    let external = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(external.status(), StatusCode::FORBIDDEN);

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let held = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(held.status(), StatusCode::NOT_FOUND);
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false,access_level='adult' WHERE id={book}"),
    );
    let adult = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        adult.status(),
        StatusCode::NOT_FOUND,
        "anonymous users cannot read restricted PACK editions"
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic',visibility='draft' WHERE id={book}"),
    );
    let hidden = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    pool.close().await;
}
