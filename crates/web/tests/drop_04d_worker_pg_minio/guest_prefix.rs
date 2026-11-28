//! Real PG/MinIO: prepare one exact SINGLE or PACK source from a live claim.
use super::support::{pg, Fixture, BUCKET};
use tor_books::{
    repositories::guest_prefix::{
        GuestPrefixPreparation, GuestPrefixSource, PgGuestPrefix, GUEST_PREFIX_CONSUMER,
        GUEST_PREFIX_EVENT,
    },
    services::authoring::AuthorError,
};
use tor_ingest::repositories::outbox::PgOutbox;
use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
use tor_web::{
    chapter_import_worker::ChapterImportWorkerResult, guest_prefix_worker::prepare_verified_job,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 and loopback MinIO only"]
async fn auth_read_claimed_sources_single_and_pack_are_exact_bodyless() {
    for packed in [false, true] {
        let label = if packed {
            "Guest prefix PACK"
        } else {
            "Guest prefix SINGLE"
        };
        let f = Fixture::new(label).await;
        let source = format!(
            "# Глава 1: Начало\nVisible first beginning. {} FIRST_PRIVATE_TAIL\n\n# Глава 2: Далее\nVisible second beginning. {} SECOND_PRIVATE_TAIL",
            "слово ".repeat(120),
            "文字 ".repeat(160),
        );
        f.enqueue(&source).await;
        assert_eq!(
            f.worker(packed).run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed
        );
        assert_eq!(f.count("chapter_publications"), 2);
        assert_eq!(
            f.count("free_chapter_storage_objects"),
            if packed { 0 } else { 2 }
        );
        // Existing isolated import fixture starts as a draft. Only this test
        // book is made basic/public and prepared; rollout remains closed.
        pg(
            &f.url,
            &format!(
                "UPDATE books SET visibility='public',access_level='basic',
                                  guest_policy_revision=1 WHERE id={}",
                f.book
            ),
        );
        let repo = PgGuestPrefix::new(f.catalog.pool().clone());
        let outbox = PgOutbox::new(f.catalog.pool().clone());
        let store = S3FreeChapterStore::new(f.client.clone(), BUCKET.to_owned()).unwrap();
        let page = repo.enqueue_missing_page(f.book, 1, 0, 64).await.unwrap();
        assert_eq!(page.enqueued, 2);
        assert_eq!(page.next_after_chapter_no, None);
        for _ in 0..2 {
            let claim = outbox
                .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
                .await
                .unwrap()
                .unwrap();
            let job = repo
                .claimed_job(&claim.id, claim.lease_epoch)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.target().book_id, f.book);
            let plan = repo.claimed_source(&job).await.unwrap().unwrap();
            assert!(match plan {
                GuestPrefixSource::Pack { .. } => packed,
                GuestPrefixSource::Single(_) => !packed,
                GuestPrefixSource::Legacy(_) => false,
            });
            assert_eq!(
                prepare_verified_job(&repo, &job, &store, "tor-other-private-ci").await,
                Err(AuthorError::Unavailable)
            );
            assert_eq!(
                prepare_verified_job(&repo, &job, &store, BUCKET).await,
                Ok(Some(GuestPrefixPreparation::Stored))
            );
            assert_eq!(
                prepare_verified_job(&repo, &job, &store, BUCKET).await,
                Ok(Some(GuestPrefixPreparation::AlreadyStored))
            );
            let text = pg(
                &f.url,
                &format!(
                    "SELECT prefix_text FROM chapter_version_guest_prefixes WHERE version_id={}",
                    job.target().version_id
                ),
            );
            assert!(text.contains("Visible") && text.contains("beginning."));
            assert!(!text.contains("PRIVATE_TAIL"));
            assert!(text.chars().count() <= 2_000 && text.len() <= 8_000);
            assert!(outbox
                .acknowledge(&claim.id, claim.lease_epoch, GUEST_PREFIX_CONSUMER)
                .await
                .unwrap());
        }
        assert!(outbox
            .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            pg(
                &f.url,
                &format!(
                    "SELECT count(*) FROM chapter_versions v JOIN chapters c ON c.id=v.chapter_id
                     WHERE c.book_id={} AND v.body_markdown IS NOT NULL",
                    f.book
                )
            ),
            "0"
        );
        assert_eq!(
            pg(
                &f.url,
                &format!(
                    "SELECT count(*) FROM chapter_version_guest_prefixes g
                     JOIN chapters c ON c.id=g.chapter_id WHERE c.book_id={}",
                    f.book
                )
            ),
            "2"
        );
    }
}
