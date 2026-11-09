//! S349: real author form, PostgreSQL18, private MinIO and mobile Chromium.
//! Uses only disposable loopback services; no old text, production or mocks.
#![cfg(feature = "s3-free")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    io::Write,
    num::NonZeroU32,
    process::{Command, Stdio},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog},
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_reading::repositories::PgReader;
use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
use tor_web::free_author_publish::PgFreeAuthorPublisher;

const ENDPOINT: &str = "http://127.0.0.1:9000";
const ORIGIN: &str = "http://127.0.0.1:3098";
const BUCKET: &str = "tor-s349-new-free-ci";
const MARKER: &str = "S349_NEW_BODY_ONLY_IN_S3_47b935";
const MARKDOWN: &str =
    "S349_NEW_BODY_ONLY_IN_S3_47b935\n\nNew chapter with Unicode: \u{1f600} \u{6587}";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:"));
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
        .expect("isolated PostgreSQL client");
    assert!(output.status.success(), "fixture SQL failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn browser(config: Value) -> Value {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/free_author_s3_pg/browser.cjs");
    let mut child = Command::new("node")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("real Chromium harness");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(config.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PG18/MinIO/Chromium from tor-paid-grant-e2e.yml"]
async fn s349_author_form_bodyless_pg_direct_browser_and_tamper_denial() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    assert_eq!(env::var("TOR_FREE_S3_PUBLIC_ORIGIN").unwrap(), ENDPOINT);
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "s349-loopback-test",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let session = generate_session_token();
    let owner = accounts
        .register_with_session(
            "s349-browser-owner@tor-ci.example",
            "S349 author",
            &hash_password("isolated-free-s3-password").unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level) \
         VALUES('ci:s349:browser-new','ru','S349 new book','public',{owner},'basic') RETURNING id"
    )).parse().unwrap();
    let publisher = PgFreeAuthorPublisher::new(
        PgFreeS3Publication::new(pool.clone()),
        store.clone(),
        BUCKET.to_owned(),
    )
    .unwrap();
    let grants = tor_web::free_reader_http::router(
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        store,
        BUCKET.to_owned(),
        ENDPOINT.to_owned(),
    )
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth_free_s3(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        (false, ORIGIN.to_owned()),
        None,
        publisher,
    )
    .merge(tor_web::instrument_additional_routes(grants));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let published = browser(json!({
        "mode": "publish", "origin": ORIGIN, "book": book,
        "session": session, "markdown": MARKDOWN,
    }));
    let chapter: i64 = published["chapter_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(published["replay"], true);
    assert_eq!(published["csrf_denied"], true);
    let version: i64 = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
        "SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter} AND body_markdown IS NULL"
    )
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter}")
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id={chapter}"
            )
        ),
        "1"
    );
    let object_key = format!("private/free-chapters/{chapter}/{version}.md");
    let bytes = client
        .get_object()
        .bucket(BUCKET)
        .key(&object_key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(bytes.as_ref(), MARKDOWN.as_bytes());
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    assert_eq!(
        http.get(format!("{ENDPOINT}/{BUCKET}/{object_key}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    let read = browser(json!({
        "mode": "read", "origin": ORIGIN, "storage_origin": ENDPOINT,
        "chapter": chapter, "markdown": MARKDOWN, "marker": MARKER,
    }));
    assert_eq!(read["read"], true);
    assert!(read["direct_s3_gets"].as_u64().unwrap() >= 1);
    let grant_url = format!("{ORIGIN}/api/v1/chapters/{chapter}/free-grant");
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        http.get(&grant_url).send().await.unwrap().status().as_u16(),
        404
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    assert_eq!(
        http.get(&grant_url).send().await.unwrap().status().as_u16(),
        200
    );

    // Same size and forged original HEAD metadata must not bypass browser SHA.
    let digest = hex::encode(Sha256::digest(MARKDOWN.as_bytes()));
    client
        .put_object()
        .bucket(BUCKET)
        .key(&object_key)
        .content_type("text/plain; charset=utf-8")
        .metadata("sha256", &digest)
        .body(ByteStream::from(vec![b'x'; MARKDOWN.len()]))
        .send()
        .await
        .unwrap();
    let damaged = browser(json!({
        "mode": "corrupt", "origin": ORIGIN, "storage_origin": ENDPOINT,
        "chapter": chapter, "marker": MARKER,
    }));
    assert_eq!(damaged["corrupt_rejected"], true);

    // A failed private S3 PUT may leave hidden metadata, never a published body.
    let unavailable = PgFreeAuthorPublisher::new(
        PgFreeS3Publication::new(pool.clone()),
        S3FreeChapterStore::new(client, "tor-s349-missing-ci".to_owned()).unwrap(),
        "tor-s349-missing-ci".to_owned(),
    )
    .unwrap();
    let command_key = "q".repeat(43);
    let input = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 2,
        expected_revision: 0,
        title: "Failed upload",
        body: "Must not become visible",
        access: "free",
        price_amount: 0,
        command_key: &command_key,
    })
    .unwrap();
    assert_eq!(
        unavailable.publish(owner, &input).await,
        Err(AuthorError::Unavailable)
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapters c JOIN chapter_versions v ON v.chapter_id=c.id \
         WHERE c.book_id={book} AND c.chapter_no=2 AND c.visibility='draft' \
         AND v.body_markdown IS NULL AND NOT EXISTS \
         (SELECT 1 FROM chapter_publications p WHERE p.chapter_id=c.id)"
            )
        ),
        "1"
    );
    server.abort();
    let _ = server.await;
    println!("S349_E2E: author form, bodyless PG, immutable retry, CSRF, direct S3 browser, hold denial, corrupted SHA and S3 outage passed");
    pool.close().await;
}
