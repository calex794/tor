//! FND-07 / S023: multi-book import and byte-for-byte accounting after restart.
#[path = "catalog_formats.rs"]
mod formats;
#[path = "catalog_provider.rs"]
mod provider;

use super::support::{insert_book, pg, Fixture};
use aws_sdk_s3::Client;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, time::Duration};
use tor_accounts::repositories::PgAccounts;
use tor_books::{
    repositories::PgCatalog,
    services::chapter_file_import::{fingerprint, parse_text, ImportedChapter},
};
use tor_imports::services::ChapterImportObjectStore;
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{FreeChapterObject, FreeChapterStore},
};
use tor_web::chapter_import_worker::ChapterImportWorkerResult;
use tower::ServiceExt;

const LOOPBACK_BUCKET: &str = "tor-fnd07-import-ci";
const LOOPBACK_ENDPOINT: &str = "http://127.0.0.1:9000";
const BOOKS: usize = 10;
const CHAPTERS_PER_BOOK: i64 = 3;

struct Import {
    book: i64,
    job: i64,
    key: String,
    hash: [u8; 32],
    bytes: i64,
    chapters: Vec<ImportedChapter>,
}

#[derive(serde::Deserialize)]
struct Edition {
    chapter_id: i64,
    version_id: i64,
    chapter_no: i32,
    title: String,
    version_no: i32,
    bodyless: bool,
    published: bool,
    edition_hash: String,
    object_hash: String,
    bucket: String,
    object_key: String,
    size_bytes: i64,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires dedicated test PostgreSQL and S3"]
async fn fnd07_ten_books_survive_worker_restart_and_reconcile_exact_s3_bytes() {
    let label = "FND-07 book 1";
    let (mut fixture, bucket, endpoint) = match provider::requested_fixture(label).await {
        Some((fixture, bucket)) => (fixture, bucket, provider::YANDEX_ENDPOINT),
        None => (
            provider::loopback_fixture(label, LOOPBACK_BUCKET).await,
            LOOPBACK_BUCKET.into(),
            LOOPBACK_ENDPOINT,
        ),
    };
    let mut imports = Vec::new();
    for index in 1..=BOOKS {
        let book = if index == 1 {
            fixture.book
        } else {
            insert_book(&fixture.url, fixture.actor, &format!("FND-07 book {index}"))
        };
        let mut source = String::new();
        for chapter in 1..=CHAPTERS_PER_BOOK {
            source.push_str(&format!(
                "# Глава {chapter}: Книга {index}\nТекст {index}/{chapter} 中文 😀\n\n"
            ));
        }
        let chapters = parse_text("chapters.txt", &source).unwrap();
        assert_eq!(chapters.len() as i64, CHAPTERS_PER_BOOK);
        let hash = Sha256::digest(source.as_bytes()).into();
        let bytes = source.len() as i64;
        let (job, key) = fixture.enqueue_for_book(book, &source).await;
        let uploaded = fixture
            .source
            .get_verified(&key, bytes, &hash, source.len())
            .await
            .unwrap();
        assert_eq!(uploaded.as_slice(), source.as_bytes());
        imports.push(Import {
            book,
            job,
            key,
            hash,
            bytes,
            chapters,
        });
    }

    let initial_worker = fixture.worker(false);
    for _ in 0..BOOKS / 2 {
        assert_eq!(
            initial_worker.run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed
        );
    }
    drop(initial_worker);
    fixture.catalog.close_for_test().await;
    fixture.catalog = PgCatalog::connect(&fixture.url).await.unwrap();
    let resumed_worker = fixture.worker(false);
    for _ in BOOKS / 2..BOOKS {
        assert_eq!(
            resumed_worker.run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed
        );
    }
    assert_eq!(
        resumed_worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Idle
    );

    let pool = fixture.catalog.pool();
    let catalog_counts: (i64, i64, i64, i64) = query_json(&fixture, &format!(
        "SELECT json_build_array(count(DISTINCT b.id),count(DISTINCT c.id),count(v.id),count(p.version_id))
         FROM books b LEFT JOIN chapters c ON c.book_id=b.id
         LEFT JOIN chapter_versions v ON v.chapter_id=c.id
         LEFT JOIN chapter_publications p ON p.chapter_id=c.id AND p.version_id=v.id
         WHERE b.owner_user_id={}", fixture.actor,
    ));
    let chapters = BOOKS as i64 * CHAPTERS_PER_BOOK;
    assert_eq!(catalog_counts, (BOOKS as i64, chapters, chapters, chapters));

    // Age gating must not account for the anonymous denial on draft books.
    pg(
        &fixture.url,
        &format!(
            "UPDATE books SET access_level='basic' WHERE owner_user_id={}",
            fixture.actor,
        ),
    );
    let (store, reader_bucket) = fixture.author().reader_store();
    let router = tor_web::free_reader_http::router(
        PgAccounts::from_pool(pool.clone()),
        false,
        "http://127.0.0.1:3098".into(),
        store.clone(),
        reader_bucket,
        endpoint.into(),
    )
    .unwrap();
    let mut expected_keys = BTreeSet::new();
    for import in &imports {
        verify_import(&fixture, import).await;
        let editions = published_editions(&fixture, import.book);
        assert_eq!(
            editions.len(),
            import.chapters.len(),
            "book {}",
            import.book
        );
        for (edition, expected) in editions.iter().zip(&import.chapters) {
            assert!(expected_keys.insert(verify_edition(&store, &bucket, edition, expected).await));
            let request = format!("/api/v1/chapters/{}/free-grant", edition.chapter_id);
            let (status, _) = get(&router, &request).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "draft book leaked a public grant"
            );
        }
    }
    assert_eq!(
        list_all_keys(&fixture.client, &bucket).await,
        expected_keys,
        "missing or unexpected S3 keys"
    );

    // After publication the same S349 reader must select the verified receipt.
    pg(
        &fixture.url,
        &format!(
            "UPDATE books SET visibility='public' WHERE id={}",
            fixture.book
        ),
    );
    let (chapter, version): (i64, i64) = query_json(
        &fixture,
        &format!(
            "SELECT json_build_array(c.id,p.version_id)
         FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE c.book_id={} AND c.chapter_no=1",
            fixture.book,
        ),
    );
    let (status, grant) = get(&router, &format!("/api/v1/chapters/{chapter}/free-grant")).await;
    assert_eq!(status, StatusCode::OK);
    let grant: serde_json::Value = serde_json::from_slice(&grant).unwrap();
    assert_eq!(grant["version_id"], version.to_string());
    let body = &imports[0].chapters[0].body;
    assert_eq!(
        grant["sha256"],
        hex::encode(Sha256::digest(body.as_bytes()))
    );
    assert_eq!(grant["size_bytes"], body.len() as u64);
    let key = FreeChapterObject::for_markdown(chapter, version, body)
        .unwrap()
        .key();
    verify_reader_download(
        endpoint,
        &bucket,
        &key,
        grant["url"].as_str().unwrap(),
        body,
    )
    .await;
    if endpoint == LOOPBACK_ENDPOINT {
        assert_eq!(
            provider::check_bucket(&fixture.client, &bucket).await,
            Err(provider::BUCKET_NOT_EMPTY)
        );
    }
    println!("FND07_MULTI_BOOK_IMPORT_OK books={BOOKS} chapters={chapters} versions={chapters} objects={} missing=0 unexpected=0", expected_keys.len());
    if endpoint == provider::YANDEX_ENDPOINT {
        println!("FND07_YANDEX_FREE_CATALOG_OK provider=Yandex versioning=never-enabled signed_download=verified");
    }
}

fn published_editions(fixture: &Fixture, book: i64) -> Vec<Edition> {
    query_json(
        fixture,
        &format!(
            "SELECT coalesce(json_agg(e ORDER BY e.chapter_no),'[]') FROM (
                SELECT c.id AS chapter_id,v.id AS version_id,c.chapter_no,v.version_no,c.title,
                       v.body_markdown IS NULL AS bodyless, p.version_id=v.id AS published,
                       encode(v.content_sha256,'hex') AS edition_hash,
                       encode(o.sha256,'hex') AS object_hash,o.bucket,o.object_key,o.size_bytes
                FROM chapters c JOIN chapter_versions v ON v.chapter_id=c.id
                JOIN chapter_publications p ON p.chapter_id=c.id
                JOIN free_chapter_storage_objects o ON o.chapter_id=c.id AND o.version_id=v.id
                WHERE c.book_id={book}
            ) e",
        ),
    )
}

async fn verify_import(fixture: &Fixture, import: &Import) {
    let job: (String, i32, String, i64) = query_json(
        fixture,
        &format!(
            "SELECT json_build_array(status,saved_count,encode(source_sha256,'hex'),source_bytes)
         FROM chapter_import_jobs WHERE id={}",
            import.job,
        ),
    );
    assert_eq!(
        job,
        (
            "succeeded".into(),
            CHAPTERS_PER_BOOK as i32,
            hex::encode(import.hash),
            import.bytes
        )
    );
    let command_key = import.key.rsplit('/').next().unwrap();
    assert_eq!(
        fixture
            .jobs()
            .existing_for_command(
                fixture.actor,
                import.book,
                command_key,
                "chapters.txt",
                &import.hash,
                import.bytes,
                &fingerprint(import.book, &import.chapters),
                CHAPTERS_PER_BOOK as i32,
            )
            .await
            .unwrap(),
        Some(import.job),
        "a completed command must replay after staging cleanup",
    );
}

async fn verify_edition(
    store: &S3FreeChapterStore,
    bucket: &str,
    edition: &Edition,
    expected: &ImportedChapter,
) -> String {
    let object =
        FreeChapterObject::for_markdown(edition.chapter_id, edition.version_id, &expected.body)
            .unwrap();
    assert_eq!(edition.chapter_no, expected.number);
    assert_eq!(edition.title, expected.title);
    assert_eq!(edition.version_no, 1);
    assert!(edition.bodyless && edition.published);
    assert_eq!(edition.bucket, bucket);
    assert_eq!(edition.object_key, object.key());
    let expected_hash = hex::encode(object.sha256());
    assert_eq!(edition.edition_hash, expected_hash);
    assert_eq!(edition.object_hash, expected_hash);
    assert_eq!(edition.size_bytes as u64, object.size_bytes());
    let bytes = store.get_verified(&object).await.unwrap();
    assert_eq!(bytes.as_slice(), expected.body.as_bytes());
    edition.object_key.clone()
}

fn query_json<T: serde::de::DeserializeOwned>(fixture: &Fixture, query: &str) -> T {
    serde_json::from_str(&pg(&fixture.url, query)).expect("typed catalog evidence")
}

async fn get(router: &Router, route: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::builder().uri(route).body(Body::empty()).unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    (
        response.status(),
        to_bytes(response.into_body(), 16_384)
            .await
            .unwrap()
            .to_vec(),
    )
}

async fn list_all_keys(client: &Client, bucket: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut pages = client
        .list_objects_v2()
        .bucket(bucket)
        .max_keys(7)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        for object in page.unwrap().contents() {
            assert!(keys.insert(object.key().expect("S3 key").to_owned()));
        }
        assert!(keys.len() <= BOOKS * CHAPTERS_PER_BOOK as usize);
    }
    keys
}

async fn verify_reader_download(endpoint: &str, bucket: &str, key: &str, grant: &str, body: &str) {
    let object = reqwest::Url::parse(&format!("{endpoint}/{bucket}/{key}")).unwrap();
    let signed = reqwest::Url::parse(grant).expect("valid signed object URL");
    let mut unsigned = signed.clone();
    unsigned.set_query(None);
    assert!(
        unsigned == object
            && signed
                .query_pairs()
                .any(|(name, _)| name == "X-Amz-Signature"),
        "grant must target the exact object on the configured provider"
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let mut response = client
        .get(signed)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut bytes = Vec::with_capacity(body.len());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)
        .unwrap()
    {
        assert!(
            bytes.len() + chunk.len() <= body.len(),
            "reader returned unexpected bytes"
        );
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(bytes, body.as_bytes());
    let anonymous = client
        .get(object)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .unwrap();
    assert_eq!(
        anonymous.status(),
        StatusCode::FORBIDDEN,
        "storage must deny an unsigned read"
    );
}
