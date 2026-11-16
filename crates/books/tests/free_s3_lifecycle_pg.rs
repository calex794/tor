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
