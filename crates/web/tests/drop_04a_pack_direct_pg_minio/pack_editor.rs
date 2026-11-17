//! DROP-04c: a real author browser edits a PACK-only chapter into SINGLE V2.
//! The immutable PACK V1 and its saved-reader pin must survive the edit.
use super::{pg, BUCKET};
use aws_sdk_s3::Client;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog};
use tor_reading::repositories::{free_s3_ref::PgFreeS3Ref, PgReader};
use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
use tor_web::free_author_publish::PgFreeAuthorPublisher;

const ORIGIN: &str = "http://127.0.0.1:3098";

pub(super) async fn verify(
    url: &str,
    client: &Client,
    actor: i64,
    book: i64,
    receipt: &tor_contracts::drop_pack::PackPlacementReceipt,
    chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
) {
    let frame = &receipt.frames[1];
    let (chapter, version) = (frame.chapter_id, frame.version_id);
    let original = chapters[1].body.as_str();
    let session = generate_session_token();
    let digest = session_token_digest(&session);
    let hex_digest = hex::encode(digest);
    pg(
        url,
        &format!(
            "INSERT INTO sessions(user_id,token_hash,expires_at)
             VALUES({actor},decode('{hex_digest}','hex'),now()+interval '1 hour')"
        ),
    );
    // The browser must preserve the original V1, not merely load a public
    // source after publication has moved to V2.
    pg(
        url,
        &format!(
            "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
             VALUES({actor},{chapter},{version})"
        ),
    );
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool().clone();
    let repository = PgFreeS3Publication::new(pool.clone());
    assert!(
        repository
            .owned(actor, book, chapter)
            .await
            .unwrap()
            .unwrap()
            .stored_in_pack
    );
    assert!(repository
        .owned(actor + 999_999, book, chapter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={chapter} AND version_id={version}"
            ),
        ),
        "0",
        "PACK V1 cannot be backed by a fake SINGLE"
    );
    let accounts = PgAccounts::from_pool(pool.clone());
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.into()).unwrap();
    let publisher = PgFreeAuthorPublisher::new(repository, store, BUCKET.into()).unwrap();
    let app = tor_web::router_with_postgres_reading_auth_free_s3(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        (false, ORIGIN.into()),
        None,
        publisher,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let changed = format!("{original}\n\nDROP04C_EDITED_SINGLE_V2");
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/free_author_s3_pg/browser.cjs");
    let mut child = Command::new("node")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({
                "mode": "edit",
                "origin": ORIGIN,
                "book": book,
                "chapter": chapter,
                "session": session,
                "previous": original,
                "markdown": changed,
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "PACK-only browser edit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["edited"], true);
    assert_eq!(result["replay"], true);

    let current: i64 = pg(
        url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap();
    assert_ne!(current, version);
    let key = format!("private/free-chapters/{chapter}/{current}.md");
    let single = client
        .get_object()
        .bucket(BUCKET)
        .key(key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(single.as_ref(), changed.as_bytes());
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter}
                 AND body_markdown IS NOT NULL"
            ),
        ),
        "0",
        "edited source never enters PostgreSQL"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={chapter} AND version_id={version}"
            ),
        ),
        "0",
        "the edit must not materialize an obsolete V1 SINGLE"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_edition_active_storage
                 WHERE chapter_id={chapter} AND version_id={version}"
            ),
        ),
        "1",
        "the pinned V1 generation remains in PACK"
    );
    let saved = PgFreeS3Ref::new(pool.clone())
        .for_saved_active_pack_session(chapter, version, Some(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.version_id, version);
    assert_eq!(saved.sha256, Sha256::digest(original.as_bytes()).into());
    println!(
        "DROP04C_PACK_EDITOR_PASS: owner browser V1 PACK -> V2 SINGLE, replay, pin preservation"
    );
    server.abort();
    let _ = server.await;
    pool.close().await;
}
