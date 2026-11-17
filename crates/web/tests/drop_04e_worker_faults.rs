//! DROP-04e: actual worker, actual PG18/MinIO, bounded injected storage/SQL failures.
#![cfg(all(feature = "s3-free", feature = "s3-chapter-import"))]
#[path = "drop_04e_worker_faults/proxy.rs"]
mod proxy;
#[path = "drop_04d_worker_pg_minio/support.rs"]
mod support;
use proxy::{Fault, Relay};
use std::sync::Arc;
use support::{pg, Fixture, BUCKET};
use tor_web::chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult as Outcome};

const SOURCE: &str =
    "Глава 1: Первая\nПроверка транзакции 😀\n\nГлава 2: Вторая\nПроверка отката 中文";
fn worker(f: &Fixture, relay: &Relay) -> ChapterImportWorker {
    let client = aws_sdk_s3::Client::from_conf(
        f.client
            .config()
            .to_builder()
            .endpoint_url(&relay.endpoint)
            .build(),
    );
    ChapterImportWorker::new(&f.catalog, f.source.clone())
        .with_free_s3(f.author().with_direct_pack_import(client))
}
fn retry_now(f: &Fixture, job: i64) {
    pg(&f.url,&format!("UPDATE chapter_import_jobs SET locked_until=clock_timestamp()-interval '1 second',run_after=clock_timestamp()-interval '1 second' WHERE id={job}"));
}
fn unchanged(f: &Fixture) {
    assert_eq!(f.count("chapters"), 0);
    assert_eq!(f.count("chapter_versions"), 0);
    assert_eq!(f.count("chapter_publications"), 0);
    assert_eq!(f.count("free_edition_active_storage"), 0);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and MinIO only"]
async fn drop_04e_real_pack_io_faults_never_publish_partial_editions() {
    for fault in [
        Fault::BeforePut,
        Fault::PackPutAck,
        Fault::ManifestPutAck,
        Fault::FirstGet,
        Fault::SecondGet,
        Fault::CorruptGet,
        Fault::StealLease,
        Fault::HoldAfterStage,
    ] {
        let f = Fixture::new("Injected PACK fault").await;
        let (job, source) = f.enqueue(SOURCE).await;
        let relay = Relay::start(fault, &f.url, job, f.book).await;
        let result = worker(&f, &relay).run_one().await.unwrap();
        assert!(
            relay.hits() > 0,
            "fault must actually be reached: {fault:?}"
        );
        if matches!(fault, Fault::PackPutAck | Fault::ManifestPutAck) {
            assert_eq!(
                result,
                Outcome::Completed,
                "an independently verified upload recovers lost ACK"
            );
        } else {
            unchanged(&f);
            if matches!(fault, Fault::HoldAfterStage) {
                assert_eq!(result, Outcome::Failed);
                assert_eq!(pg(&f.url,&format!("SELECT count(*) FROM free_pack_direct_import_stages WHERE job_id={job}")),"1");
                assert_eq!(
                    pg(
                        &f.url,
                        &format!("SELECT status FROM chapter_import_jobs WHERE id={job}")
                    ),
                    "failed"
                );
                continue;
            }
            assert_eq!(
                result,
                if matches!(fault, Fault::StealLease) {
                    Outcome::Stale
                } else {
                    Outcome::Retried
                },
                "{fault:?}"
            );
            assert!(
                f.client
                    .head_object()
                    .bucket(BUCKET)
                    .key(&source)
                    .send()
                    .await
                    .is_ok(),
                "retry cannot delete source"
            );
            retry_now(&f, job);
            assert_eq!(
                f.worker(false).run_one().await.unwrap(),
                Outcome::Completed,
                "disabled rollout still resumes selected PACK: {fault:?}"
            );
        }
        assert_eq!(f.count("chapters"), 2);
        assert_eq!(f.count("chapter_versions"), 2);
        assert_eq!(f.count("chapter_publications"), 2);
        assert_eq!(f.count("free_edition_active_storage"), 2);
        assert_eq!(f.count("free_chapter_storage_objects"), 0);
        assert_eq!(
            pg(
                &f.url,
                &format!("SELECT saved_count FROM chapter_import_jobs WHERE id={job}")
            ),
            "2"
        );
        println!("DROP04E_PACK_FAULT_PASS {fault:?}");
    }
}

// A real PostgreSQL statement failure after actual MinIO I/O. The app role
// stays unchanged; only the test admin may install/remove this scoped trigger.
fn admin(sql: &str) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(std::env::var("PGHOST").as_deref(), Ok("127.0.0.1"));
    assert_eq!(std::env::var("PGUSER").as_deref(), Ok("postgres"));
    let output = std::process::Command::new("psql")
        .args([
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            "tor_test",
            "--command",
            sql,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "CI SQL fault fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable CI PG18/MinIO and fixture admin only"]
async fn drop_04e_pg_stage_and_mid_batch_failures_roll_back_every_publication() {
    for table in ["free_pack_direct_import_stages", "chapter_publications"] {
        let f = Fixture::new("Injected PG fault").await;
        let (job, source) = f.enqueue(SOURCE).await;
        let predicate = if table == "chapter_publications" {
            format!("EXISTS(SELECT 1 FROM chapters c WHERE c.id=NEW.chapter_id AND c.book_id={} AND c.chapter_no=2)",f.book)
        } else {
            format!("NEW.job_id={job}")
        };
        admin(&format!("CREATE SEQUENCE drop04e_fault_hits; GRANT USAGE,SELECT ON SEQUENCE drop04e_fault_hits TO tor_app; CREATE FUNCTION drop04e_fail_write() RETURNS trigger LANGUAGE plpgsql AS $fault$ BEGIN IF {predicate} THEN PERFORM nextval('drop04e_fault_hits'); RAISE EXCEPTION 'CI simulated database outage' USING ERRCODE='08006'; END IF; RETURN NEW; END; $fault$; CREATE TRIGGER drop04e_fail_write BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION drop04e_fail_write();"));
        let result = f.worker(true).run_one().await;
        let hit = pg(&f.url, "SELECT is_called FROM drop04e_fault_hits");
        admin(&format!("DROP TRIGGER drop04e_fail_write ON {table}; DROP FUNCTION drop04e_fail_write(); DROP SEQUENCE drop04e_fault_hits;"));
        assert_eq!(result.unwrap(), Outcome::Retried);
        assert_eq!(hit, "t", "the injected SQL failure must actually execute");
        unchanged(&f);
        assert_eq!(
            pg(
                &f.url,
                &format!("SELECT count(*) FROM free_pack_direct_import_stages WHERE job_id={job}")
            ),
            if table == "chapter_publications" {
                "1"
            } else {
                "0"
            }
        );
        assert!(f
            .client
            .head_object()
            .bucket(BUCKET)
            .key(&source)
            .send()
            .await
            .is_ok());
        retry_now(&f, job);
        assert_eq!(f.worker(false).run_one().await.unwrap(), Outcome::Completed);
        assert_eq!(f.count("chapter_publications"), 2);
        assert_eq!(f.count("chapter_versions"), 2);
        assert_eq!(f.count("free_chapter_storage_objects"), 0);
        println!("DROP04E_PG_FAULT_PASS {table}");
    }
}

struct CleanupUnavailable(Arc<tor_imports::repositories::s3::S3ChapterImportStore>);
#[async_trait::async_trait]
impl tor_imports::services::ChapterImportObjectStore for CleanupUnavailable {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        hash: &[u8; 32],
    ) -> Result<(), tor_imports::services::ChapterImportStoreError> {
        self.0.put_verified(key, bytes, hash).await
    }
    async fn get_verified(
        &self,
        key: &str,
        size: i64,
        hash: &[u8; 32],
        limit: usize,
    ) -> Result<Vec<u8>, tor_imports::services::ChapterImportStoreError> {
        self.0.get_verified(key, size, hash, limit).await
    }
    async fn delete(
        &self,
        _key: &str,
    ) -> Result<(), tor_imports::services::ChapterImportStoreError> {
        Err(tor_imports::services::ChapterImportStoreError::Unavailable)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and MinIO only"]
async fn drop_04e_cleanup_failure_keeps_committed_pack_and_does_not_reimport() {
    let f = Fixture::new("Cleanup failure").await;
    let (job, source) = f.enqueue(SOURCE).await;
    let worker =
        ChapterImportWorker::new(&f.catalog, Arc::new(CleanupUnavailable(f.source.clone())))
            .with_free_s3(f.author().with_direct_pack_import(f.client.clone()));
    assert_eq!(worker.run_one().await.unwrap(), Outcome::Completed);
    assert_eq!(worker.run_one().await.unwrap(), Outcome::Idle);
    assert_eq!(f.count("chapter_publications"), 2);
    assert_eq!(f.count("chapter_versions"), 2);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT status FROM chapter_import_jobs WHERE id={job}")
        ),
        "succeeded"
    );
    assert!(f
        .client
        .head_object()
        .bucket(BUCKET)
        .key(&source)
        .send()
        .await
        .is_ok());
    println!(
        "DROP04E_CLEANUP_FAULT_PASS: durable success, inert source orphan, no duplicate import"
    );
}
