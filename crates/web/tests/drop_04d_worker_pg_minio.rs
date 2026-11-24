//! Real S023 queue -> private S3 source -> PACK-only publication, bounded batches.
#![cfg(all(feature = "s3-free", feature = "s3-chapter-import"))]
#[cfg(all(feature = "s3-covers", not(feature = "s3-paid")))]
#[path = "drop_04d_worker_pg_minio/candidate_profile.rs"]
mod candidate_profile;
#[path = "drop_04d_worker_pg_minio/catalog_concurrency.rs"]
mod catalog_concurrency;
#[path = "drop_04d_worker_pg_minio/catalog_import.rs"]
mod catalog_import;
#[path = "drop_04d_worker_pg_minio/support.rs"]
mod support;
use std::time::Instant;
use support::{pg, Fixture, BUCKET};
use tor_books::services::chapter_file_import::{parse_text, MAX_IMPORT_BYTES};
use tor_storage::repositories::free_pack_write_s3::S3FreePackWriter;
use tor_web::chapter_import_worker::ChapterImportWorkerResult;

const SOURCE: &str = "# Глава 1: Начало\nПервый текст 😀\n\n# Глава 2: Далее\nВторой текст 中文";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and loopback MinIO only"]
async fn drop_04d_real_worker_recovers_staged_pack_after_lease_expiry() {
    let f = Fixture::new("PACK worker retry").await;
    let (job, source_key) = f.enqueue(SOURCE).await;
    let claim = f.jobs().claim_next().await.unwrap().unwrap();
    assert_eq!(claim.job_id, job);
    let chapters = parse_text("chapters.txt", SOURCE).unwrap();
    let receipt = f
        .author()
        .stage_direct_pack_only(&claim, &chapters, &S3FreePackWriter::new(f.client.clone()))
        .await
        .unwrap();
    assert_eq!(f.count("chapters"), 0, "staged upload cannot publish");
    // Crash after durable stage, before finalize. A replacement claims a new
    // epoch and reconstructs the same immutable bytes, not another publication.
    pg(&f.url, &format!("UPDATE chapter_import_jobs SET locked_until=clock_timestamp()-interval '1 second' WHERE id={job}"));
    let worker = f.worker(false);
    assert_eq!(
        worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Completed
    );
    assert_eq!(
        worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Idle
    );
    assert_eq!(pg(&f.url, &format!("SELECT status||':'||saved_count||':'||lease_epoch FROM chapter_import_jobs WHERE id={job}")), "succeeded:2:2");
    assert_eq!(f.count("chapters"), 2);
    assert_eq!(f.count("chapter_versions"), 2);
    assert_eq!(f.count("chapter_publications"), 2);
    assert_eq!(f.count("free_edition_active_storage"), 2);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT transport FROM free_chapter_import_transports WHERE job_id={job}")
        ),
        "pack",
        "disabling rollout cannot change a staged PACK job"
    );
    assert_eq!(pg(&f.url, &format!("SELECT count(*) FROM chapter_versions v JOIN chapters c ON c.id=v.chapter_id WHERE c.book_id={} AND v.body_markdown IS NOT NULL", f.book)), "0");
    let stored = pg(
        &f.url,
        &format!("SELECT receipt::text FROM free_pack_direct_import_stages WHERE job_id={job}"),
    );
    assert_eq!(
        serde_json::from_str::<tor_contracts::drop_pack::PackPlacementReceipt>(&stored).unwrap(),
        receipt
    );
    let missing = f
        .client
        .head_object()
        .bucket(BUCKET)
        .key(source_key)
        .send()
        .await
        .unwrap_err();
    assert_eq!(
        missing.as_service_error().map(|e| e.meta().code()),
        Some(Some("NotFound"))
    );
    println!("DROP04D_WORKER_RETRY_PASS: reclaimed stage, exact PACK, no SINGLE, atomic job");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and loopback MinIO only"]
async fn drop_04d_worker_keeps_single_default_and_denies_held_book() {
    let f = Fixture::new("SINGLE default").await;
    f.enqueue(SOURCE).await;
    assert_eq!(
        f.worker(false).run_one().await.unwrap(),
        ChapterImportWorkerResult::Completed
    );
    assert_eq!(f.count("free_chapter_storage_objects"), 2);
    assert_eq!(f.count("chapters"), 2);
    let held = Fixture::new("Held PACK import").await;
    let (job, _) = held.enqueue(SOURCE).await;
    pg(
        &held.url,
        &format!("UPDATE books SET admin_hold=true WHERE id={}", held.book),
    );
    assert_eq!(
        held.worker(true).run_one().await.unwrap(),
        ChapterImportWorkerResult::Failed
    );
    assert_eq!(held.count("chapters"), 0);
    assert_eq!(
        pg(
            &held.url,
            &format!("SELECT status FROM chapter_import_jobs WHERE id={job}")
        ),
        "failed"
    );
}

/// Explicit 100k exercise, not a unit-test proxy for the real queue. The
/// operator chooses the count; every batch retains the 700k/500 S023 bounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit TOR_DROP_LOAD_CHAPTERS=100000 with disposable PG18/MinIO"]
async fn drop_04d_100k_bounded_worker_import() {
    let total: usize = std::env::var("TOR_DROP_LOAD_CHAPTERS")
        .expect("explicit load count")
        .parse()
        .unwrap();
    assert!((1..=100_000).contains(&total));
    let f = Fixture::new("Bounded 100k PACK load").await;
    let worker = f.worker(true);
    let rss_before = linux_rss("VmRSS:");
    let cpu_before = linux_cpu_ticks();
    let mut worker_durations = Vec::new();
    let started = Instant::now();
    let mut batches = 0;
    let body = "Синтетический текст испытания хранилища 😀 中文. ".repeat(11);
    for start in (0..total).step_by(500) {
        let count = 500.min(total - start);
        let mut source = String::new();
        for number in (start + 1)..=(start + count) {
            source.push_str(&format!(
                "# Глава {number}: Испытание {number}\n{body}{number}\n\n"
            ));
        }
        assert!(source.len() <= MAX_IMPORT_BYTES);
        let (job, _) = f.enqueue(&source).await;
        let batch_started = Instant::now();
        assert_eq!(
            worker.run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed,
            "batch {batches}"
        );
        worker_durations.push(batch_started.elapsed().as_secs_f64());
        assert_eq!(
            pg(
                &f.url,
                &format!("SELECT saved_count FROM chapter_import_jobs WHERE id={job}")
            ),
            count.to_string()
        );
        batches += 1;
        if batches % 20 == 0 {
            println!(
                "DROP_LOAD_PROGRESS chapters={} batches={batches} seconds={:.2}",
                start + count,
                started.elapsed().as_secs_f64()
            );
        }
    }
    assert_eq!(f.count("chapters"), total as i64);
    assert_eq!(f.count("chapter_publications"), total as i64);
    assert_eq!(f.count("chapter_versions"), total as i64);
    assert_eq!(f.count("free_edition_active_storage"), total as i64);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
    assert_eq!(
        worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Idle
    );
    assert_eq!(pg(&f.url, &format!("SELECT count(*) FROM chapter_versions v JOIN chapters c ON c.id=v.chapter_id WHERE c.book_id={} AND v.body_markdown IS NOT NULL", f.book)), "0");
    println!(
        "DROP04D_LOAD_PASS chapters={total} batches={batches} seconds={:.3}",
        started.elapsed().as_secs_f64()
    );
    worker_durations.sort_by(f64::total_cmp);
    let p95 = worker_durations[(worker_durations.len() * 95).div_ceil(100) - 1];
    let p99 = worker_durations[(worker_durations.len() * 99).div_ceil(100) - 1];
    let ticks_per_second = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    assert!(ticks_per_second.status.success());
    let ticks_per_second: f64 = String::from_utf8(ticks_per_second.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let cpu_seconds = (linux_cpu_ticks() - cpu_before) as f64 / ticks_per_second;
    println!("DROP04E_TIMING_PASS worker_batch_p95_s={p95:.6} worker_batch_p99_s={p99:.6} client_cpu_s={cpu_seconds:.3}");
    let peak = linux_rss("VmHWM:");
    println!("DROP04E_RESOURCE_PASS client_peak_rss_kib={peak} before_kib={rss_before} peak_growth_kib={}", peak.saturating_sub(rss_before));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and MinIO only"]
async fn drop_04d_persisted_pack_never_falls_back_when_reader_or_flag_changes() {
    use tor_books::repositories::free_s3_publish::{
        FreeImportTransport, ImportTransportRequest, PgFreeS3Publication,
    };
    use tor_web::chapter_import_worker::ChapterImportWorker;
    let f = Fixture::new("PACK choice before upload").await;
    let (job, source_key) = f.enqueue(SOURCE).await;
    let claim = f.jobs().claim_next().await.unwrap().unwrap();
    let chapters = parse_text("chapters.txt", SOURCE).unwrap();
    let selected = PgFreeS3Publication::new(f.catalog.pool().clone())
        .select_import_transport(ImportTransportRequest {
            job: (claim.job_id, claim.epoch),
            actor: f.actor,
            book: f.book,
            command_key: &claim.command_key,
            chapters: &chapters,
            prefer_pack: true,
        })
        .await
        .unwrap();
    assert_eq!(selected, FreeImportTransport::Pack);
    pg(&f.url,&format!("UPDATE chapter_import_jobs SET locked_until=clock_timestamp()-interval '1 second' WHERE id={job}"));
    // A runtime lacking the PACK client cannot silently switch this job to
    // SINGLE. Its retry preserves both the source and immutable choice.
    let missing_reader =
        ChapterImportWorker::new(&f.catalog, f.source.clone()).with_free_s3(f.author());
    assert_eq!(
        missing_reader.run_one().await.unwrap(),
        ChapterImportWorkerResult::Retried
    );
    assert_eq!(f.count("chapters"), 0);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
    assert!(f
        .client
        .head_object()
        .bucket(BUCKET)
        .key(&source_key)
        .send()
        .await
        .is_ok());
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT transport FROM free_chapter_import_transports WHERE job_id={job}")
        ),
        "pack"
    );
    pg(&f.url,&format!("UPDATE chapter_import_jobs SET run_after=clock_timestamp()-interval '1 second' WHERE id={job}"));
    assert_eq!(
        f.worker(false).run_one().await.unwrap(),
        ChapterImportWorkerResult::Completed
    );
    assert_eq!(f.count("chapter_publications"), 2);
    assert_eq!(f.count("free_chapter_storage_objects"), 0);
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT lease_epoch FROM chapter_import_jobs WHERE id={job}")
        ),
        "3"
    );
    println!(
        "DROP04D_PACK_POLICY_PASS: pre-upload choice survives missing reader and disabled rollout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and MinIO only"]
async fn drop_04d_legacy_single_checkpoint_survives_pack_rollout() {
    use tor_books::repositories::free_s3_publish::{
        PgFreeS3Publication, VerifiedFreeImportEdition,
    };
    use tor_storage::{
        repositories::free_chapter_s3::S3FreeChapterStore,
        services::free_chapter::{upload_and_verify, FreeChapterObject},
    };
    let f = Fixture::new("Legacy SINGLE checkpoint").await;
    let (job, _) = f.enqueue(SOURCE).await;
    let claim = f.jobs().claim_next().await.unwrap().unwrap();
    let chapters = parse_text("chapters.txt", SOURCE).unwrap();
    let repo = PgFreeS3Publication::new(f.catalog.pool().clone());
    let reserved = repo
        .prepare_import(f.actor, f.book, &claim.command_key, &chapters)
        .await
        .unwrap();
    let first = &reserved[0];
    let object =
        FreeChapterObject::for_markdown(first.chapter_id, first.version_id, &chapters[0].body)
            .unwrap();
    let store = S3FreeChapterStore::new(f.client.clone(), BUCKET.into()).unwrap();
    upload_and_verify(&store, &object, &chapters[0].body)
        .await
        .unwrap();
    repo.mark_import_verified(
        f.actor,
        &claim.command_key,
        BUCKET,
        &VerifiedFreeImportEdition {
            chapter_id: first.chapter_id,
            version_id: first.version_id,
            sha256: *object.sha256(),
            size_bytes: object.size_bytes(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT count(*) FROM free_chapter_import_transports WHERE job_id={job}")
        ),
        "0"
    );
    pg(&f.url,&format!("UPDATE chapter_import_jobs SET locked_until=clock_timestamp()-interval '1 second' WHERE id={job}"));
    assert_eq!(
        f.worker(true).run_one().await.unwrap(),
        ChapterImportWorkerResult::Completed
    );
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT transport FROM free_chapter_import_transports WHERE job_id={job}")
        ),
        "single"
    );
    assert_eq!(f.count("free_chapter_storage_objects"), 2);
    assert_eq!(f.count("chapter_publications"), 2);
    assert_eq!(
        pg(
            &f.url,
            &format!("SELECT count(*) FROM free_pack_direct_import_stages WHERE job_id={job}")
        ),
        "0"
    );
    println!(
        "DROP04D_LEGACY_SINGLE_PASS: real verified checkpoint resumes SINGLE after PACK rollout"
    );
}

// Linux CI measurement of the actual worker process. Includes runtime and
// shared-client overhead; not a claim about total PostgreSQL/MinIO/VPS memory.
fn linux_rss(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|value| value.split_whitespace().next())
        .unwrap()
        .parse()
        .unwrap()
}

fn linux_cpu_ticks() -> u64 {
    let status = std::fs::read_to_string("/proc/self/stat").unwrap();
    let mut fields = status.rsplit_once(") ").unwrap().1.split_whitespace();
    let user: u64 = fields.nth(11).unwrap().parse().unwrap();
    let system: u64 = fields.next().unwrap().parse().unwrap();
    user + system
}
