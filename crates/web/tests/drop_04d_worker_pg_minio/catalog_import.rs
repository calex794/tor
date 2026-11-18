//! FND-07 / S023: multi-book import and byte-for-byte accounting after restart.
use super::support::{pg, Fixture};
use aws_sdk_s3::Client;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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

const BUCKET: &str = "tor-fnd07-import-ci";
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
#[ignore = "requires the disposable S023 PostgreSQL and MinIO fixture"]
async fn fnd07_ten_books_survive_worker_restart_and_reconcile_exact_s3_bytes() {
    let mut fixture = Fixture::new_in_bucket("FND-07 book 1", BUCKET).await;
    let mut imports = Vec::new();
    for index in 1..=BOOKS {
        let book = if index == 1 {
            fixture.book
        } else {
            fixture.create_book(&format!("FND-07 book {index}"))
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
    let (store, bucket) = fixture.author().reader_store();
    let router = tor_web::free_reader_http::router(
        PgAccounts::from_pool(pool.clone()),
        false,
        "http://127.0.0.1:3098".into(),
        store.clone(),
        bucket,
        "http://127.0.0.1:9000".into(),
    )
    .unwrap();
    let mut expected_keys = BTreeSet::new();
    for import in &imports {
        verify_import(&fixture, import).await;
        let editions: Vec<Edition> = query_json(
            &fixture,
            &format!(
                "SELECT coalesce(json_agg(e ORDER BY e.chapter_no),'[]') FROM (
                SELECT c.id AS chapter_id,v.id AS version_id,c.chapter_no,v.version_no,c.title,
                       v.body_markdown IS NULL AS bodyless, p.version_id=v.id AS published,
                       encode(v.content_sha256,'hex') AS edition_hash,
                       encode(o.sha256,'hex') AS object_hash,o.bucket,o.object_key,o.size_bytes
                FROM chapters c JOIN chapter_versions v ON v.chapter_id=c.id
                JOIN chapter_publications p ON p.chapter_id=c.id
                JOIN free_chapter_storage_objects o ON o.chapter_id=c.id AND o.version_id=v.id
                WHERE c.book_id={}
            ) e",
                import.book,
            ),
        );
        assert_eq!(
            editions.len(),
            import.chapters.len(),
            "book {}",
            import.book
        );
        for (edition, expected) in editions.iter().zip(&import.chapters) {
            assert!(expected_keys.insert(verify_edition(&store, edition, expected).await));
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
        list_all_keys(&fixture.client).await,
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
    println!("FND07_MULTI_BOOK_IMPORT_OK books={BOOKS} chapters={chapters} versions={chapters} objects={} missing=0 unexpected=0", expected_keys.len());
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
    assert_eq!(edition.bucket, BUCKET);
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

async fn list_all_keys(client: &Client) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut pages = client
        .list_objects_v2()
        .bucket(BUCKET)
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
