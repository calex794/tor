//! Real S023 queue -> private S3 source -> PACK-only publication, bounded batches.
#![cfg(all(feature = "s3-free", feature = "s3-chapter-import"))]
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
    let worker = f.worker(true);
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
        assert_eq!(
            worker.run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed,
            "batch {batches}"
        );
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
}
