//! Real disposable PG18: immutable S3 metadata, CAS, release and atomic import.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::{
        free_s3_publish::{PgFreeS3Publication, VerifiedFreeImportEdition},
        postgres::PgAuthoring,
    },
    services::{
        authoring::{save_chapter, AuthorError, SaveChapter, SaveChapterRequest},
        chapter_file_import::ImportedChapter,
    },
};

async fn fixture(name: &str, visibility: &str) -> (PgPool, i64, i64, PgFreeS3Publication) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    let pool = PgPool::connect(&url).await.unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
        VALUES($1,'S349 lifecycle','$argon2id$test') RETURNING id",
    )
    .bind(format!("s349-{name}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
        VALUES($1,'ru','S349 lifecycle',$2,$3) RETURNING id",
    )
    .bind(format!("ci:s349:{name}"))
    .bind(visibility)
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    let store = PgFreeS3Publication::new(pool.clone());
    (pool, actor, book, store)
}
fn request(
    book: i64,
    chapter: Option<i64>,
    revision: i64,
    number: i32,
    key: char,
    text: &str,
) -> SaveChapter {
    save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: chapter.map(|n| n as u64),
        expected_revision: revision,
        number,
        title: "S3 chapter",
        body: text,
        access: "free",
        price_amount: 0,
        command_key: &key.to_string().repeat(43),
    })
    .unwrap()
}
fn metadata(text: &str) -> (&'static str, [u8; 32], u64) {
    (
        "tor-private-s349-ci",
        Sha256::digest(text.as_bytes()).into(),
        text.len() as u64,
    )
}

async fn job(
    pool: &PgPool,
    actor: i64,
    book: i64,
    key: &str,
    sources: &[ImportedChapter],
    epoch: i64,
) -> i64 {
    let fingerprint = tor_books::services::chapter_file_import::fingerprint(book, sources);
    sqlx::query_scalar("INSERT INTO chapter_import_jobs
        (book_id,actor_user_id,command_key,source_key,source_filename,source_sha256,source_bytes,
         approved_fingerprint,expected_chapter_count,status,lease_epoch,locked_until,attempts)
        VALUES($1,$2,$3,$4,'chapters.txt',$5,100,$6,$7,'running',$8,now()+interval '2 minutes',1) RETURNING id")
        .bind(book).bind(actor).bind(key).bind(format!("staging/chapters/s349-reserved-{key}"))
        .bind([1u8;32].as_slice()).bind(fingerprint.as_slice()).bind(sources.len() as i32).bind(epoch)
        .fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn s349_cancelled_import_does_not_occupy_numbers_and_stale_epoch_never_commits() {
    let (pool, actor, book, store) = fixture("job-fence", "draft").await;
    let sources = vec![ImportedChapter {
        number: 1,
        title: "S3 chapter".to_owned(),
        body: "Only S3😀".to_owned(),
    }];
    let key = "u".repeat(43);
    let reserved = store
        .prepare_import(actor, book, &key, &sources)
        .await
        .unwrap();
    let (_, sha256, size_bytes) = metadata(&sources[0].body);
    let editions = vec![VerifiedFreeImportEdition {
        chapter_id: reserved[0].chapter_id,
        version_id: reserved[0].version_id,
        sha256,
        size_bytes,
    }];
    store
        .mark_import_verified(actor, &key, "tor-private-s349-ci", &editions[0])
        .await
        .unwrap();
    assert!(
        store
            .prepare_import(actor, book, &key, &sources)
            .await
            .unwrap()[0]
            .verified
    );
    let first = job(&pool, actor, book, &key, &sources, 2).await;
    assert!(matches!(
        store
            .finish_import_job((first, 1), &sources, "tor-private-s349-ci", &editions)
            .await,
        Err(AuthorError::Conflict)
    ));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapters WHERE book_id=$1",
            book
        )
        .await,
        0
    );
    sqlx::query("UPDATE chapter_import_jobs SET status='cancelled',locked_until=NULL WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .finish_import_job((first, 2), &sources, "tor-private-s349-ci", &editions)
            .await,
        Err(AuthorError::Conflict)
    ));
    let next_key = "v".repeat(43);
    let next = store
        .prepare_import(actor, book, &next_key, &sources)
        .await
        .unwrap();
    let next_editions = vec![VerifiedFreeImportEdition {
        chapter_id: next[0].chapter_id,
        version_id: next[0].version_id,
        sha256,
        size_bytes,
    }];
    store
        .mark_import_verified(actor, &next_key, "tor-private-s349-ci", &next_editions[0])
        .await
        .unwrap();
    let second = job(&pool, actor, book, &next_key, &sources, 3).await;
    assert_eq!(
        store
            .finish_import_job((second, 3), &sources, "tor-private-s349-ci", &next_editions)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapters WHERE book_id=$1",
            book
        )
        .await,
        1
    );
    let status: String = sqlx::query_scalar("SELECT status FROM chapter_import_jobs WHERE id=$1")
        .bind(second)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "succeeded");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapter_versions WHERE chapter_id=$1",
            reserved[0].chapter_id
        )
        .await,
        0
    );
    pool.close().await;
}
async fn count(pool: &PgPool, query: &str, chapter: i64) -> i64 {
    sqlx::query_scalar(query)
        .bind(chapter)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn s349_edit_retains_v1_until_verified_commit_and_rejects_stale_writer() {
    let (pool, actor, book, store) = fixture("edit", "public").await;
    let input = request(book, None, 0, 1, 'j', "First😀\n\nEnd");
    let first = store.prepare_new(actor, &input).await.unwrap();
    store
        .attach_verified_and_publish(
            actor,
            book,
            (first.chapter_id, first.version_id),
            metadata(&input.body),
        )
        .await
        .unwrap();
    let edit = request(
        book,
        Some(first.chapter_id),
        1,
        1,
        'k',
        "Second **edition**😀",
    );
    let next = store.prepare_edit(actor, &edit).await.unwrap();
    assert!(!next.published);
    assert_eq!(
        count(
            &pool,
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1",
            first.chapter_id
        )
        .await,
        first.version_id
    );
    assert!(matches!(
        store.prepare_edit(actor + 999, &edit).await,
        Err(AuthorError::Missing)
    ));
    store
        .attach_edit(actor, &edit, &next, metadata(&edit.body))
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1",
            first.chapter_id
        )
        .await,
        next.version_id
    );
    assert!(store.prepare_edit(actor, &edit).await.unwrap().published);
    let stale = request(book, Some(first.chapter_id), 1, 1, 'l', "Stale overwrite");
    assert!(matches!(
        store.prepare_edit(actor, &stale).await,
        Err(AuthorError::Conflict)
    ));
    assert_eq!(count(&pool,"SELECT count(*) FROM chapter_versions WHERE chapter_id=$1 AND body_markdown IS NOT NULL",first.chapter_id).await,0);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapter_version_content_shapes WHERE chapter_id=$1",
            first.chapter_id
        )
        .await,
        2
    );
    assert_eq!(count(&pool,"SELECT count(*) FROM outbox_events WHERE aggregate_id=$1 AND event_type='ChapterPublished'",first.chapter_id).await,1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn s349_delayed_edition_has_no_reader_pointer_until_release() {
    let (pool, actor, book, store) = fixture("schedule", "public").await;
    sqlx::query("INSERT INTO book_delayed_publication_settings
        (book_id,enabled,interval_days,anchor_date,schedule_revision,next_slot_at,next_slot_index,updated_by)
        VALUES($1,true,1,current_date,1,now()+interval '1 day',0,$2)")
        .bind(book).bind(actor).execute(&pool).await.unwrap();
    let input = request(book, None, 0, 1, 'm', "Scheduled😀");
    let pending = store.prepare_staged(actor, &input).await.unwrap();
    store
        .attach_verified_and_stage(
            actor,
            book,
            (pending.chapter_id, pending.version_id),
            metadata(&input.body),
        )
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapter_publications WHERE chapter_id=$1",
            pending.chapter_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1",
            pending.chapter_id
        )
        .await,
        0
    );
    assert!(store.prepare_staged(actor, &input).await.unwrap().published);
    let author = PgAuthoring::new(pool.clone());
    author
        .release_pending_free_chapter(actor, book, pending.chapter_id, 1, pending.version_id)
        .await
        .unwrap();
    author
        .release_pending_free_chapter(actor, book, pending.chapter_id, 1, pending.version_id)
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1",
            pending.chapter_id
        )
        .await,
        1
    );
    assert_eq!(count(&pool,"SELECT count(*) FROM outbox_events WHERE aggregate_id=$1 AND event_type='ChapterPublished'",pending.chapter_id).await,1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn s349_import_commits_all_editions_or_none_and_replays_once() {
    let (pool, actor, book, store) = fixture("batch", "draft").await;
    let sources = vec![
        ImportedChapter {
            number: 1,
            title: "S3 chapter".to_owned(),
            body: "First".to_owned(),
        },
        ImportedChapter {
            number: 2,
            title: "S3 chapter".to_owned(),
            body: "Second😀".to_owned(),
        },
    ];
    let key = "p".repeat(43);
    let pending = store
        .prepare_import(actor, book, &key, &sources)
        .await
        .unwrap();
    let mut editions = Vec::new();
    for (source, ids) in sources.iter().zip(pending) {
        let (_, sha256, size_bytes) = metadata(&source.body);
        let edition = VerifiedFreeImportEdition {
            chapter_id: ids.chapter_id,
            version_id: ids.version_id,
            sha256,
            size_bytes,
        };
        store
            .mark_import_verified(actor, &key, "tor-private-s349-ci", &edition)
            .await
            .unwrap();
        editions.push(edition);
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM chapters WHERE book_id=$1",
            book
        )
        .await,
        0
    );
    let correct = editions[1].sha256;
    editions[1].sha256 = [9; 32];
    assert!(store
        .finish_import(
            actor,
            book,
            &key,
            &sources,
            "tor-private-s349-ci",
            &editions
        )
        .await
        .is_err());
    assert_eq!(count(&pool,"SELECT count(*) FROM chapter_publications p JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id=$1",book).await,0);
    editions[1].sha256 = correct;
    assert_eq!(
        store
            .finish_import(
                actor,
                book,
                &key,
                &sources,
                "tor-private-s349-ci",
                &editions
            )
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .finish_import(
                actor,
                book,
                &key,
                &sources,
                "tor-private-s349-ci",
                &editions
            )
            .await
            .unwrap(),
        2
    );
    assert_eq!(count(&pool,"SELECT count(*) FROM chapter_publications p JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id=$1",book).await,2);
    assert_eq!(count(&pool,"SELECT count(*) FROM chapter_versions v JOIN chapters c ON c.id=v.chapter_id WHERE c.book_id=$1 AND v.body_markdown IS NOT NULL",book).await,0);
    pool.close().await;
}

/// DROP-R4a: PG18 tests B's atomic candidate-only transaction. This is a
/// deliberately synthetic NEUTRAL receipt, NOT proof of S3 bytes. G's real
/// full GET and seekable manifest are covered by storage's MinIO E2E; R4b
/// must combine the two in one end-to-end orchestrator.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn drop_r4a_verified_pack_is_lease_fenced_and_keeps_single_reader_unchanged() {
    use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
    use tor_contracts::drop_pack::{PackPlacementFrame, PackPlacementReceipt, PackPlacementTarget};

    let (pool, actor, book, author) = fixture("drop-r4a", "public").await;
    let texts = ["Первая глава, Unicode 😀", "Вторая глава, право V1 中文"];
    let mut published = Vec::new();
    let mut frames = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        let input = request(
            book,
            None,
            0,
            (index + 1) as i32,
            (b'q' + index as u8) as char,
            text,
        );
        let ids = author.prepare_new(actor, &input).await.unwrap();
        author
            .attach_verified_and_publish(
                actor,
                book,
                (ids.chapter_id, ids.version_id),
                metadata(text),
            )
            .await
            .unwrap();
        frames.push(PackPlacementFrame {
            chapter_id: ids.chapter_id,
            version_id: ids.version_id,
            content_sha256: metadata(text).1,
            size_bytes: text.len() as u64,
            frame_offset: (index * 1_000) as u64,
            frame_bytes: 500,
        });
        published.push(ids);
    }
    let pack_key = format!("private/free-packs/ci/drop-r4a-{book}.zst");
    let receipt = PackPlacementReceipt {
        target: PackPlacementTarget {
            bucket: "tor-private-drop-r4a-ci".to_owned(),
            pack_key: pack_key.clone(),
            manifest_key: pack_key.replace(".zst", ".manifest.json"),
            pack_sha256: [31; 32],
            manifest_sha256: [41; 32],
            pack_bytes: 16_000,
            manifest_bytes: 512,
            seek_table_start: 15_000,
        },
        frames,
    };
    let repo = PgFreePackCatalog::new(pool.clone());
    let approved = PgFreePackCatalog::fingerprint(&receipt);
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO free_pack_jobs
         (actor_user_id,book_id,approved_fingerprint,expected_chapter_count,
          status,lease_epoch,locked_until)
         VALUES($1,$2,$3,2,'running',7,now()+interval '2 minutes') RETURNING id",
    )
    .bind(actor)
    .bind(book)
    .bind(approved.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repo.finish_pack_job(job, 6, &receipt).await,
        Err(AuthorError::Conflict)
    ));

    sqlx::query("UPDATE free_pack_jobs SET locked_until=now()-interval '1 second' WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.finish_pack_job(job, 7, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    sqlx::query("UPDATE free_pack_jobs SET status='cancelled',locked_until=NULL WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.finish_pack_job(job, 7, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    sqlx::query(
        "UPDATE free_pack_jobs SET status='running',locked_until=now()+interval '2 minutes'
         WHERE id=$1",
    )
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE free_pack_jobs SET approved_fingerprint=$2 WHERE id=$1")
        .bind(job)
        .bind([92u8; 32].as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.finish_pack_job(job, 7, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    sqlx::query("UPDATE free_pack_jobs SET approved_fingerprint=$2 WHERE id=$1")
        .bind(job)
        .bind(approved.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.finish_pack_job(job, 7, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    // R4c serializes live PACK attempts per book. Temporarily cancel
    // the first synthetic job to exercise a competing malformed candidate.
    sqlx::query("UPDATE free_pack_jobs SET status='cancelled',locked_until=NULL WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();

    // This independently approved but invalid candidate has frame #2
    // overlapping frame #1. The R2 PG trigger fails *after* the pack and
    // first generation insert; the enclosing B transaction must roll both
    // writes back instead of leaving a partial physical catalog.
    let mut overlap = receipt.clone();
    overlap.target.pack_key = format!("private/free-packs/ci/drop-r4a-overlap-{book}.zst");
    overlap.target.manifest_key = overlap.target.pack_key.replace(".zst", ".manifest.json");
    overlap.frames[1].frame_offset = 100;
    let overlap_fingerprint = PgFreePackCatalog::fingerprint(&overlap);
    let overlap_job: i64 = sqlx::query_scalar(
        "INSERT INTO free_pack_jobs
         (actor_user_id,book_id,approved_fingerprint,expected_chapter_count,
          status,lease_epoch,locked_until)
         VALUES($1,$2,$3,2,'running',8,now()+interval '2 minutes') RETURNING id",
    )
    .bind(actor)
    .bind(book)
    .bind(overlap_fingerprint.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repo.finish_pack_job(overlap_job, 8, &overlap).await,
        Err(AuthorError::Conflict)
    ));
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM free_storage_packs WHERE object_key=$1")
            .bind(&overlap.target.pack_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        leaked, 0,
        "a bad later frame must roll back the PACK insert"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM free_pack_jobs WHERE id=$1")
        .bind(overlap_job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "running", "no partial success after rollback");

    let empty: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_edition_storage_generations
         WHERE (chapter_id,version_id) IN (($1,$2),($3,$4))",
    )
    .bind(receipt.frames[0].chapter_id)
    .bind(receipt.frames[0].version_id)
    .bind(receipt.frames[1].chapter_id)
    .bind(receipt.frames[1].version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(empty, 0);

    // The invalid attempt did not persist a PACK or candidate generation.
    // Retire its still-live lease before restoring the original R4a job.
    sqlx::query("UPDATE free_pack_jobs SET status='failed',locked_until=NULL WHERE id=$1")
        .bind(overlap_job)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE free_pack_jobs SET status='running',locked_until=now()+interval '2 minutes'
         WHERE id=$1",
    )
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();

    // Same-job concurrent retries serialize under the PG job lock. The loser
    // sees the committed pack and returns that same ID without new generations.
    let (a, b) = tokio::join!(
        repo.finish_pack_job(job, 7, &receipt),
        repo.finish_pack_job(job, 7, &receipt),
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    assert!(first > 0);
    assert_eq!(repo.finish_pack_job(job, 7, &receipt).await.unwrap(), first);
    assert!(matches!(
        repo.finish_pack_job(job, 6, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_edition_storage_generations WHERE pack_id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attached, 2);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_edition_active_storage
         WHERE (chapter_id,version_id) IN (($1,$2),($3,$4))",
    )
    .bind(receipt.frames[0].chapter_id)
    .bind(receipt.frames[0].version_id)
    .bind(receipt.frames[1].chapter_id)
    .bind(receipt.frames[1].version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 0, "R4a candidate attachment never enables PACK");
    for ids in published {
        let published_version: i64 =
            sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
                .bind(ids.chapter_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(published_version, ids.version_id);
        assert_eq!(
            count(
                &pool,
                "SELECT count(*) FROM free_chapter_storage_objects WHERE chapter_id=$1",
                ids.chapter_id,
            )
            .await,
            1
        );
    }
    let status: String = sqlx::query_scalar("SELECT status FROM free_pack_jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "succeeded");
    pool.close().await;
}


#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci; no production activation"]
async fn drop_05a_atomic_pack_cutover_adopts_implicit_single_and_fences_replay() {
    use tor_books::repositories::{
        free_pack_attach::PgFreePackCatalog, free_pack_cutover::PackActivation,
    };
    use tor_contracts::drop_pack::{PackPlacementFrame, PackPlacementReceipt, PackPlacementTarget};

    let (pool, actor, book, author) = fixture("drop-05a-first", "public").await;
    let texts = ["Первый будущий SINGLE 😀", "Второй будущий SINGLE 中文"];
    let mut frames = Vec::new();
    for (index, body) in texts.into_iter().enumerate() {
        let command = request(
            book,
            None,
            0,
            (index + 1) as i32,
            (b'k' + index as u8) as char,
            body,
        );
        let ids = author.prepare_new(actor, &command).await.unwrap();
        author
            .attach_verified_and_publish(
                actor,
                book,
                (ids.chapter_id, ids.version_id),
                metadata(body),
            )
            .await
            .unwrap();
        frames.push(PackPlacementFrame {
            chapter_id: ids.chapter_id,
            version_id: ids.version_id,
            content_sha256: metadata(body).1,
            size_bytes: body.len() as u64,
            frame_offset: (index * 1_000) as u64,
            frame_bytes: 500,
        });
    }
    let key = format!("private/free-packs/ci/drop-05a-first-{book}.zst");
    let receipt = PackPlacementReceipt {
        target: PackPlacementTarget {
            bucket: "tor-private-drop05-ci".into(),
            pack_key: key.clone(),
            manifest_key: key.replace(".zst", ".manifest.json"),
            pack_sha256: [38; 32],
            manifest_sha256: [49; 32],
            pack_bytes: 16_000,
            manifest_bytes: 512,
            seek_table_start: 15_000,
        },
        frames,
    };
    let repo = PgFreePackCatalog::new(pool.clone());
    let zero: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_edition_active_storage a
         JOIN chapters c ON c.id=a.chapter_id WHERE c.book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(zero, 0, "ordinary SINGLE publication has no active pointer");

    let job = repo.schedule_pack_job(actor, book, &receipt).await.unwrap();
    let claim = repo.claim_pack_job(120).await.unwrap().unwrap();
    assert_eq!(claim.job_id, job);
    assert_eq!(claim.receipt, receipt);
    repo.finish_pack_job(job, claim.epoch, &receipt)
        .await
        .unwrap();

    // Every member must be permitted. Deny after the first has been checked;
    // no pointer is allowed to be committed for any member.
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(receipt.frames[1].chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.activate_verified_pack(job, claim.epoch, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    let empty: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_edition_active_storage a
         JOIN chapters c ON c.id=a.chapter_id WHERE c.book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(empty, 0, "late member failure cannot partially activate a PACK");
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(receipt.frames[1].chapter_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(matches!(
        repo.activate_verified_pack(job, claim.epoch - 1, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    let mut fake = receipt.clone();
    fake.target.pack_sha256[0] ^= 1;
    assert!(matches!(
        repo.activate_verified_pack(job, claim.epoch, &fake).await,
        Err(AuthorError::Conflict)
    ));

    // Same-job competing callers must serialize and perform one logical CAS.
    let (left, right) = tokio::join!(
        repo.activate_verified_pack(job, claim.epoch, &receipt),
        repo.activate_verified_pack(job, claim.epoch, &receipt)
    );
    assert!(matches!(
        (left.unwrap(), right.unwrap()),
        (PackActivation::Activated, PackActivation::AlreadyActive)
            | (PackActivation::AlreadyActive, PackActivation::Activated)
    ));
    assert_eq!(
        repo.activate_verified_pack(job, claim.epoch, &receipt)
            .await
            .unwrap(),
        PackActivation::AlreadyActive
    );

    for f in &receipt.frames {
        let active_kind: String = sqlx::query_scalar(
            "SELECT g.placement_kind
             FROM free_edition_active_storage a
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation
             WHERE a.chapter_id=$1 AND a.version_id=$2",
        )
        .bind(f.chapter_id)
        .bind(f.version_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_kind, "pack");
        let source: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM free_chapter_storage_objects
             WHERE chapter_id=$1 AND version_id=$2",
        )
        .bind(f.chapter_id)
        .bind(f.version_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(source, 1, "original SINGLE retained for verified rollback");
    }

    // A separately advanced generation for one edition invalidates the
    // whole replay. Never reset PACK/SINGLE pointers backwards.
    let first = &receipt.frames[0];
    sqlx::query(
        "INSERT INTO free_edition_storage_generations
           (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes)
         VALUES($1,$2,2,'single',$3,$4)",
    )
    .bind(first.chapter_id)
    .bind(first.version_id)
    .bind(first.content_sha256.as_slice())
    .bind(first.size_bytes as i64)
    .execute(&pool)
    .await
    .unwrap();
    let switched: bool =
        sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,1,2)")
            .bind(first.chapter_id)
            .bind(first.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(switched);
    assert!(matches!(
        repo.activate_verified_pack(job, claim.epoch, &receipt).await,
        Err(AuthorError::Conflict)
    ));
    let second_generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM free_edition_active_storage
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(receipt.frames[1].chapter_id)
    .bind(receipt.frames[1].version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(second_generation, 1, "mixed state cannot trigger a new cutover");
    println!("DROP05A_ATOMIC_PG_PASS: implicit SINGLE, all-member hold, epoch, replay, race, retained source, no pointer rewind");
    pool.close().await;
}
