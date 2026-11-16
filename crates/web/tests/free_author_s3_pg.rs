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

#[path = "free_author_s3_pg/traffic.rs"]
mod traffic;

// Run the DROP full PACK proof+PG acceptance in the existing MinIO CI lane.
#[path = "drop_r4b1_pg_minio.rs"]
mod drop_r4b1_pg_minio;
#[path = "drop_04a_pack_direct_pg_minio.rs"]
mod drop_04a_pack_direct_pg_minio;

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
    let (app, traffic) = traffic::instrument(app, MARKER);
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
    let expired = client
        .get_object()
        .bucket(BUCKET)
        .key(&object_key)
        .presigned(
            aws_sdk_s3::presigning::PresigningConfig::builder()
                .start_time(std::time::SystemTime::now() - std::time::Duration::from_secs(180))
                .expires_in(std::time::Duration::from_secs(90))
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        http.get(expired.uri())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403,
        "an expired direct capability must not retrieve the private source"
    );
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
        "session":session,"progress":true, "width":390,
    }));
    assert_eq!(read["read"], true);
    println!("S349_NETWORK mobile={read}");
    assert_eq!(pg(&url,&format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={owner} AND chapter_id={chapter}")),"1");
    let desktop = browser(
        json!({"mode":"read","origin":ORIGIN,"storage_origin":ENDPOINT,
        "chapter":chapter,"markdown":MARKDOWN,"marker":MARKER,"width":1440}),
    );
    assert_eq!(desktop["read"], true);
    println!("S349_NETWORK desktop={desktop}");
    let no_js = browser(json!({"mode":"no-js","private":true,"origin":ORIGIN,
        "storage_origin":ENDPOINT,"chapter":chapter,"session":session,"marker":MARKER}));
    assert_eq!(no_js["read"], true);
    println!("S349_NETWORK no_javascript={no_js}");

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
    pg(&url,&format!("INSERT INTO account_roles(user_id,role,is_active) VALUES({owner},'administrator',true)"));
    let admin = browser(json!({"mode":"read","admin":true,"origin":ORIGIN,
        "storage_origin":ENDPOINT,"chapter":chapter,"session":session,"marker":MARKER}));
    assert_eq!(admin["read"], true);
    println!("S349_NETWORK hidden_admin={admin}");
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

    client
        .put_object()
        .bucket(BUCKET)
        .key(&object_key)
        .content_type("text/plain; charset=utf-8")
        .metadata("sha256", &digest)
        .body(ByteStream::from(MARKDOWN.as_bytes().to_vec()))
        .send()
        .await
        .unwrap();
    tor_reading::repositories::PgLibrary::new(pool.clone())
        .add_book(owner, book)
        .await
        .unwrap();
    // A second reader pins V1 before V2 but has not yet finished it. The owner
    // already completed V1; moving that cursor backwards must remain a conflict.
    let archive_session = generate_session_token();
    let archive_actor = PgAccounts::from_pool(pool.clone())
        .register_with_session(
            "s349-archive-reader@tor-ci.example",
            "S349 archive reader",
            &hash_password("isolated-archive-password").unwrap(),
            &session_token_digest(&archive_session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    tor_reading::repositories::PgLibrary::new(pool.clone())
        .add_book(archive_actor, book)
        .await
        .unwrap();
    let v2 = format!("{MARKDOWN}\n\nV2_ONLY_MARKER");
    let edited = browser(
        json!({"mode":"edit","origin":ORIGIN,"book":book,"chapter":chapter,
        "session":session,"previous":MARKDOWN,"markdown":v2}),
    );
    assert_eq!(edited["edited"], true);
    let issue: i64 = pg(
        &url,
        &format!("SELECT id FROM text_issues WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        tor_discussions::repositories::text_issues::PgTextIssues::new(pool.clone())
            .update_status(owner, book, issue, "fixed", Some("Published S3 V2"))
            .await
            .unwrap(),
        tor_discussions::repositories::text_issues::StatusMutation::Changed
    );
    assert_eq!(pg(&url,&format!("SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter} AND body_markdown IS NOT NULL")),"0");
    let saved = browser(
        json!({"mode":"read","saved":true,"origin":ORIGIN,"storage_origin":ENDPOINT,
        "chapter":chapter,"session":session,"markdown":MARKDOWN,"marker":MARKER,
        "version":version,"absent_marker":"V2_ONLY_MARKER"}),
    );
    assert_eq!(saved["read"], true);
    println!("S349_NETWORK saved_v1={saved}");
    let admin_history = browser(
        json!({"mode":"read","admin":true,"history":version,"origin":ORIGIN,
        "storage_origin":ENDPOINT,"chapter":chapter,"session":session,"marker":MARKER}),
    );
    println!("S349_NETWORK admin_history={admin_history}");
    assert_eq!(pg(&url,&format!("SELECT count(*) FROM reader_admin_chapter_access_audit WHERE admin_user_id={owner} AND chapter_id={chapter}")),"3");
    pg(&url,&format!("UPDATE account_roles SET is_active=false WHERE user_id={owner} AND role='administrator'"));
    assert_eq!(
        http.get(format!("{ORIGIN}/read/admin/{chapter}"))
            .header("cookie", format!("tor-dev-session={session}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    let source = http
        .get(format!(
            "{grant_url}?mode=saved&version_id={version}&format=source"
        ))
        .header("cookie", format!("tor-dev-session={session}"))
        .send()
        .await
        .unwrap();
    assert_eq!(source.status().as_u16(), 303);
    let source_url = source.headers()["location"].to_str().unwrap();
    assert!(source_url.starts_with(ENDPOINT));
    assert_eq!(
        http.get(source_url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        MARKDOWN
    );
    assert_eq!(
        http.get(format!("{grant_url}?mode=saved&version_id={version}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );

    // A failed private S3 PUT may leave hidden metadata, never a published body.
    let unavailable = PgFreeAuthorPublisher::new(
        PgFreeS3Publication::new(pool.clone()),
        S3FreeChapterStore::new(client.clone(), "tor-s349-missing-ci".to_owned()).unwrap(),
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
    // Published metadata must fail closed if its exact object disappears.
    let current: i64 = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap();
    let current_key = format!("private/free-chapters/{chapter}/{current}.md");
    client
        .delete_object()
        .bucket(BUCKET)
        .key(&current_key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        http.get(&grant_url).send().await.unwrap().status().as_u16(),
        503
    );
    client
        .put_object()
        .bucket(BUCKET)
        .key(&current_key)
        .content_type("text/plain; charset=utf-8")
        .metadata("sha256", hex::encode(Sha256::digest(v2.as_bytes())))
        .body(ByteStream::from(v2.into_bytes()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        http.get(&grant_url).send().await.unwrap().status().as_u16(),
        200
    );
    pg(
        &url,
        &format!("UPDATE chapters SET access_level='paid',price_amount=100 WHERE id={chapter}"),
    );
    assert_eq!(
        http.get(&grant_url).send().await.unwrap().status().as_u16(),
        404
    );
    assert_eq!(
        http.get(format!("{grant_url}?mode=saved&version_id={version}"))
            .header("cookie", format!("tor-dev-session={session}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    pg(&url,&format!("UPDATE books SET lifecycle_status='ordinary_deleted',ordinary_deleted_at=now(),visibility='draft' WHERE id={book}"));
    let archive = browser(
        json!({"mode":"read","saved":true,"origin":ORIGIN,"storage_origin":ENDPOINT,
        "chapter":chapter,"session":archive_session,"markdown":MARKDOWN,"marker":MARKER,"version":version,"archived":true,"absent_marker":"V2_ONLY_MARKER"}),
    );
    println!("S349_NETWORK deleted_archive={archive}");
    assert_eq!(pg(&url,&format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={archive_actor} AND chapter_id={chapter}")),"0",
        "archival progress does not mint a completion or a purchased right");
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        http.get(format!("{grant_url}?mode=saved&version_id={version}"))
            .header("cookie", format!("tor-dev-session={session}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
    traffic.assert_clean();
    server.abort();
    let _ = server.await;
    println!("S349_E2E: author form, bodyless PG, immutable retry, CSRF, direct S3 browser, hold denial, corrupted SHA and S3 outage passed");
    pool.close().await;
}

/// DROP-B06: exercise the real S349 B/G integration point, not a shadow schema.
///
/// An S3 PUT and full bounded GET/SHA succeed for V2, but a concurrent
/// administrative hold is applied before B may attach the storage receipt.
/// V1 must remain the sole reader-visible version. The same command can later
/// safely reuse those immutable S3 bytes without a duplicate publication.
///
/// This is a FREE edition test. Purchased paid V1 uses a separate encrypted
/// delivery path and must not be treated as covered by this scenario.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PG18/MinIO from TOR integration paid_e2e"]
async fn s349_drop_v2_verified_s3_rejected_by_pg_hold_keeps_free_v1_until_retry() {
    use tor_reading::repositories::free_s3_ref::PgFreeS3Ref;
    use tor_storage::services::free_chapter::{
        upload_and_verify, FreeChapterObject, FreeChapterStore,
    };

    let url = env::var("TEST_DATABASE_URL").expect("isolated PG18");
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    const TEST_BUCKET: &str = "tor-s349-drop-boundary-ci";
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "drop-s349-loopback-test",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(TEST_BUCKET)
        .send()
        .await
        .unwrap();
    let store = S3FreeChapterStore::new(client.clone(), TEST_BUCKET.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let actor: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s349-drop-verified-author@example.invalid',
                'DROP S349 verified author','$argon2id$test') RETURNING id",
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:drop:s349:verified-v2','ru','DROP verified V2','public',{actor},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let publication = PgFreeS3Publication::new(pool.clone());
    let reader = PgFreeS3Ref::new(pool.clone());

    let first_text = "DROP_V1_READER_MUST_SURVIVE\n\nРусский исходный текст 😀";
    let v1_command = "x".repeat(43);
    let first = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "DROP original",
        body: first_text,
        access: "free",
        price_amount: 0,
        command_key: &v1_command,
    })
    .unwrap();
    let prepared_v1 = publication.prepare_new(actor, &first).await.unwrap();
    let v1 = FreeChapterObject::for_markdown(
        prepared_v1.chapter_id,
        prepared_v1.version_id,
        &first.body,
    )
    .unwrap();
    upload_and_verify(&store, &v1, &first.body).await.unwrap();
    publication
        .attach_verified_and_publish(
            actor,
            book,
            (v1.chapter_id(), v1.version_id()),
            (TEST_BUCKET, *v1.sha256(), v1.size_bytes()),
        )
        .await
        .unwrap();

    let current = reader
        .for_session(v1.chapter_id(), None)
        .await
        .unwrap()
        .expect("public free edition V1 must resolve");
    assert_eq!(current.version_id, v1.version_id());
    assert_eq!(current.sha256, *v1.sha256());
    assert_eq!(
        store.get_verified(&v1).await.unwrap(),
        first_text.as_bytes()
    );

    let second_text = "DROP_V2_MUST_NOT_LEAK_EARLY\n\nДругая версия 中文";
    let v2_command = "y".repeat(43);
    let edit = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(v1.chapter_id() as u64),
        number: 1,
        expected_revision: 1,
        title: "DROP updated",
        body: second_text,
        access: "free",
        price_amount: 0,
        command_key: &v2_command,
    })
    .unwrap();
    let prepared_v2 = publication.prepare_edit(actor, &edit).await.unwrap();
    assert_eq!(prepared_v2.previous_version_id, v1.version_id());
    assert!(!prepared_v2.published);
    let v2 =
        FreeChapterObject::for_markdown(prepared_v2.chapter_id, prepared_v2.version_id, &edit.body)
            .unwrap();
    upload_and_verify(&store, &v2, &edit.body)
        .await
        .expect("actual S3 upload and byte verification must finish");

    // Simulate a real race: permission changes after S3 verification but
    // before the PostgreSQL publication transaction. B must deny attachment.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        publication
            .attach_edit(
                actor,
                &edit,
                &prepared_v2,
                (TEST_BUCKET, *v2.sha256(), v2.size_bytes()),
            )
            .await,
        Err(AuthorError::Missing),
    );
    assert!(
        reader
            .for_session(v1.chapter_id(), None)
            .await
            .unwrap()
            .is_none(),
        "book hold must prevent even the old current edition from being granted",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT version_id FROM chapter_publications
                 WHERE chapter_id={}",
                v1.chapter_id()
            ),
        ),
        v1.version_id().to_string(),
        "the V2 bytes are verified but not reader-visible",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={} AND version_id={}",
                v2.chapter_id(),
                v2.version_id()
            ),
        ),
        "0",
        "there must be no attached S3 receipt for the failed publication",
    );
    assert_eq!(
        store.get_verified(&v2).await.unwrap(),
        second_text.as_bytes(),
        "the unreferenced verified V2 object must remain private for safe retry",
    );
    assert_eq!(
        store.get_verified(&v1).await.unwrap(),
        first_text.as_bytes(),
        "existing immutable V1 must survive the failed update",
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    assert_eq!(
        reader
            .for_session(v1.chapter_id(), None)
            .await
            .unwrap()
            .unwrap()
            .version_id,
        v1.version_id(),
        "lifting the hold must not implicitly publish the pending V2",
    );
    // The existing immutable V2 key now conflicts on PUT, but the real G
    // service verifies the unchanged bytes and accepts the retry.
    upload_and_verify(&store, &v2, &edit.body)
        .await
        .expect("idempotent retry must reverify stored V2");
    publication
        .attach_edit(
            actor,
            &edit,
            &prepared_v2,
            (TEST_BUCKET, *v2.sha256(), v2.size_bytes()),
        )
        .await
        .unwrap();
    publication
        .attach_edit(
            actor,
            &edit,
            &prepared_v2,
            (TEST_BUCKET, *v2.sha256(), v2.size_bytes()),
        )
        .await
        .expect("duplicate receipt attachment must not duplicate a version");

    let new_current = reader
        .for_session(v1.chapter_id(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_current.version_id, v2.version_id());
    assert_eq!(new_current.sha256, *v2.sha256());
    assert_eq!(
        store.get_verified(&v1).await.unwrap(),
        first_text.as_bytes()
    );
    assert_eq!(
        store.get_verified(&v2).await.unwrap(),
        second_text.as_bytes()
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={}",
                v1.chapter_id()
            ),
        ),
        "2",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_versions
                 WHERE chapter_id={} AND body_markdown IS NOT NULL",
                v1.chapter_id()
            ),
        ),
        "0",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events
                 WHERE chapter_id={}",
                v1.chapter_id()
            ),
        ),
        "1",
        "editing V2 must not duplicate first-release events",
    );
    pool.close().await;
}
