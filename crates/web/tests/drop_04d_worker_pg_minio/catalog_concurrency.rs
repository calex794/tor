//! FND-03 / S023: two workers import separate books from the same durable queue.
use super::support::{insert_book, pg, Fixture};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tor_books::repositories::PgCatalog;
use tor_imports::services::{ChapterImportObjectStore, ChapterImportStoreError};
use tor_storage::services::free_chapter::{FreeChapterObject, FreeChapterStore};
use tor_web::chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult};

const BOOKS: usize = 100;
const BUCKET: &str = "tor-fnd03-concurrent-ci";

struct GatedSource {
    storage: Arc<dyn ChapterImportObjectStore>,
    arrived: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ChapterImportObjectStore for GatedSource {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        sha256: &[u8; 32],
    ) -> Result<(), ChapterImportStoreError> {
        self.storage.put_verified(key, bytes, sha256).await
    }

    async fn get_verified(
        &self,
        key: &str,
        expected_size: i64,
        sha256: &[u8; 32],
        max_bytes: usize,
    ) -> Result<Vec<u8>, ChapterImportStoreError> {
        self.arrived.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.storage
            .get_verified(key, expected_size, sha256, max_bytes)
            .await
    }

    async fn delete(&self, key: &str) -> Result<(), ChapterImportStoreError> {
        self.storage.delete(key).await
    }
}

#[derive(Deserialize)]
struct Receipt {
    book_id: i64,
    job_id: i64,
    source_hash: String,
    chapter_id: i64,
    version_id: i64,
    object_key: String,
    object_hash: String,
    edition_hash: String,
    size_bytes: u64,
    committed_once: bool,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable S023 PostgreSQL and loopback MinIO fixture"]
async fn fnd03_two_workers_import_one_hundred_books_with_exact_receipts() {
    let fixture = Fixture::new_in_bucket("FND-03 book 1", BUCKET).await;
    assert_eq!(
        pg(
            &fixture.url,
            "SELECT count(*) FROM chapter_import_jobs WHERE status IN ('queued','running')"
        ),
        "0"
    );
    let mut expected = Vec::with_capacity(BOOKS);
    let mut staging_keys = BTreeSet::new();
    for index in 1..=BOOKS {
        let book = if index == 1 {
            fixture.book
        } else {
            insert_book(&fixture.url, fixture.actor, &format!("FND-03 book {index}"))
        };
        let body = format!("Текст книги {index} 中文 😀");
        let source = format!("# Глава 1: Книга {index}\n{body}");
        let (job, key) = fixture.enqueue_for_book(book, &source).await;
        assert!(staging_keys.insert(key));
        expected.push((book, job, hex::encode(Sha256::digest(source)), body));
    }

    let second_catalog = PgCatalog::connect(&fixture.url).await.unwrap();
    let gate = Arc::new(GatedSource {
        storage: fixture.source.clone(),
        arrived: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let first =
        ChapterImportWorker::new(&fixture.catalog, gate.clone()).with_free_s3(fixture.author());
    let second =
        ChapterImportWorker::new(&second_catalog, gate.clone()).with_free_s3(fixture.author());
    let (left, right, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(first.run_one(), second.run_one(), async {
            gate.arrived.acquire_many(2).await.unwrap().forget();
            let state = pg(
                &fixture.url,
                &format!(
                    "SELECT json_build_array(
                       (SELECT count(*) FROM chapter_import_jobs WHERE actor_user_id={0}
                        AND status='running' AND attempts=1 AND lease_epoch=1),
                       (SELECT count(*) FROM chapters c JOIN books b ON b.id=c.book_id
                        WHERE b.owner_user_id={0}))",
                    fixture.actor,
                ),
            );
            assert_eq!(serde_json::from_str::<(i64, i64)>(&state).unwrap(), (2, 0));
            gate.release.add_permits(2);
        })
    })
    .await
    .expect("two distinct leases must reach source reads before either can publish");
    assert_eq!(left.unwrap(), ChapterImportWorkerResult::Completed);
    assert_eq!(right.unwrap(), ChapterImportWorkerResult::Completed);

    let first = fixture.worker(false);
    let second = ChapterImportWorker::new(&second_catalog, fixture.source.clone())
        .with_free_s3(fixture.author());
    for _ in 1..BOOKS / 2 {
        let (left, right) = tokio::join!(first.run_one(), second.run_one());
        assert_eq!(left.unwrap(), ChapterImportWorkerResult::Completed);
        assert_eq!(right.unwrap(), ChapterImportWorkerResult::Completed);
    }
    let (left, right) = tokio::join!(first.run_one(), second.run_one());
    assert_eq!(left.unwrap(), ChapterImportWorkerResult::Idle);
    assert_eq!(right.unwrap(), ChapterImportWorkerResult::Idle);
    verify_catalog(&fixture, &expected).await;
    println!(
        "FND03_CONCURRENT_CATALOG_OK books={BOOKS} editions={BOOKS} objects={BOOKS} workers=2"
    );
}

async fn verify_catalog(fixture: &Fixture, expected: &[(i64, i64, String, String)]) {
    let rows = pg(
        &fixture.url,
        &format!(
            "SELECT coalesce(json_agg(r ORDER BY r.book_id),'[]') FROM (
               SELECT b.id AS book_id,j.id AS job_id,encode(j.source_sha256,'hex') AS source_hash,
                      c.id AS chapter_id,v.id AS version_id,o.object_key,o.size_bytes,
                      encode(o.sha256,'hex') AS object_hash,encode(v.content_sha256,'hex') AS edition_hash,
                      j.status='succeeded' AND j.saved_count=1 AND j.attempts=1 AND j.lease_epoch=1
                         AND j.locked_until IS NULL AND c.chapter_no=1 AND v.version_no=1
                         AND v.body_markdown IS NULL AND p.version_id=v.id
                         AND o.bucket='{BUCKET}' AS committed_once
               FROM books b LEFT JOIN chapter_import_jobs j ON j.book_id=b.id
               LEFT JOIN chapters c ON c.book_id=b.id
               LEFT JOIN chapter_versions v ON v.chapter_id=c.id
               LEFT JOIN chapter_publications p ON p.chapter_id=c.id
               LEFT JOIN free_chapter_storage_objects o ON o.chapter_id=c.id AND o.version_id=v.id
               WHERE b.owner_user_id={}
             ) r",
            fixture.actor
        ),
    );
    let receipts: Vec<Receipt> = serde_json::from_str(&rows).expect("complete catalog receipts");
    assert_eq!(receipts.len(), expected.len());
    let (store, _) = fixture.author().reader_store();
    let mut keys = BTreeSet::new();
    for (receipt, (book, job, source_hash, body)) in receipts.iter().zip(expected) {
        assert_eq!((receipt.book_id, receipt.job_id), (*book, *job));
        assert!(receipt.committed_once, "book {book} job {job}");
        assert_eq!(&receipt.source_hash, source_hash);
        let object =
            FreeChapterObject::for_markdown(receipt.chapter_id, receipt.version_id, body).unwrap();
        assert_eq!(receipt.object_key, object.key());
        assert_eq!(receipt.size_bytes, object.size_bytes());
        let digest = hex::encode(object.sha256());
        assert_eq!(receipt.object_hash, digest);
        assert_eq!(receipt.edition_hash, digest);
        assert_eq!(store.get_verified(&object).await.unwrap(), body.as_bytes());
        assert!(keys.insert(receipt.object_key.clone()));
    }
    let mut pages = fixture
        .client
        .list_objects_v2()
        .bucket(BUCKET)
        .max_keys(17)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        for object in page.unwrap().contents() {
            assert!(keys.remove(object.key().unwrap()), "unexpected S3 key");
        }
    }
    assert!(keys.is_empty(), "missing S3 keys");
}
