//! D-001 / BR-060: real B repository operations must serialize publication
//! with the immutable Layer FREE archive snapshot. S3 receipts are disposable
//! test metadata; this is not a replacement for the provider E2E tests.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;
use tor_books::{
    repositories::{free_s3_publish::PgFreeS3Publication, postgres::PgAuthoring},
    services::authoring::{save_chapter, AuthorError, SaveChapter, SaveChapterRequest},
};

fn save(book: i64, chapter: Option<i64>, revision: i64, body: &str, key: char) -> SaveChapter {
    save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: chapter.map(|id| id as u64),
        expected_revision: revision,
        number: 1,
        title: "D001 archive publication race",
        body,
        access: "free",
        price_amount: 0,
        command_key: &key.to_string().repeat(43),
    })
    .unwrap()
}

fn receipt(body: &str) -> (&'static str, [u8; 32], u64) {
    (
        "tor-private-s349-ci",
        Sha256::digest(body.as_bytes()).into(),
        body.len() as u64,
    )
}

async fn wait_blocked_by(pool: &PgPool, blocker: i32) -> i32 {
    for _ in 0..160 {
        let blocked: Option<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity
             WHERE pid<>$1 AND $1=ANY(pg_blocking_pids(pid))
             ORDER BY pid LIMIT 1",
        )
        .bind(blocker)
        .fetch_optional(pool)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("expected publication/archive lock wait was not observed");
}

async fn verify_receipt(pool: &PgPool, chapter: i64, version: i64, body: &str) {
    let expected: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let (version_sha, object_sha): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT v.content_sha256,f.sha256
         FROM chapter_versions v
         JOIN free_chapter_storage_objects f
           ON f.chapter_id=v.chapter_id AND f.version_id=v.id
         WHERE v.chapter_id=$1 AND v.id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(version_sha.as_slice(), expected.as_slice());
    assert_eq!(object_sha.as_slice(), expected.as_slice());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d001_layer_archive_waits_for_author_publication_and_keeps_saved_v1() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = "$argon2id$d001-archive-ci-fixture";
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 owner',$2) RETURNING id",
    )
    .bind(format!("d001-archive-owner-{nonce}@example.invalid"))
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 reader',$2) RETURNING id",
    )
    .bind(format!("d001-archive-reader-{nonce}@example.invalid"))
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut token = [0_u8; 32];
    token[..16].copy_from_slice(&nonce.to_le_bytes());
    token[16..].copy_from_slice(&nonce.to_le_bytes());
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(owner)
    .bind(token.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','D001 archive race','public',$2,'basic') RETURNING id",
    )
    .bind(format!("ci:d001:archive-race:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();

    let store = PgFreeS3Publication::new(pool.clone());
    let initial = save(book, None, 0, "D001 exact opened V1", 'A');
    let v1 = store.prepare_new(owner, &initial).await.unwrap();
    store
        .attach_verified_and_publish(
            owner,
            book,
            (v1.chapter_id, v1.version_id),
            receipt(&initial.body),
        )
        .await
        .unwrap();
    let chapter = v1.chapter_id;

    // Seed an already-established C-08 right with explicit exact open
    // evidence; a bare bookmark must never manufacture all FREE pins.
    // The actual open/save path is tested separately in C-08 #1185.
    sqlx::query(
        "INSERT INTO reader_free_opened_editions(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(reader)
    .bind(chapter)
    .bind(v1.version_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO reader_library_items(user_id,book_id) VALUES($1,$2)")
        .bind(reader)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         SELECT user_id,chapter_id,version_id FROM reader_free_opened_editions
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(reader)
    .bind(chapter)
    .execute(&pool)
    .await
    .unwrap();

    let input = save(book, Some(chapter), 1, "D001 replacement V2", 'B');
    let v2 = store.prepare_edit(owner, &input).await.unwrap();
    assert!(!v2.published);
    assert_ne!(v1.version_id, v2.version_id);
    let version2 = v2.version_id;
    let expected_v2_body = input.body.clone();

    // Hold only the Chapter row. The real attach_edit first locks the
    // Book, then waits on this Chapter. The real deletion request must
    // therefore wait for that author transaction, not snapshot stale V1.
    let mut guard = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    let locked: i64 = sqlx::query_scalar("SELECT id FROM chapters WHERE id=$1 FOR UPDATE")
        .bind(chapter)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(locked, chapter);
    let editor = tokio::spawn(async move {
        store
            .attach_edit(owner, &input, &v2, receipt(&input.body))
            .await
    });
    let editor_pid = wait_blocked_by(&pool, blocker).await;

    let repo = PgAuthoring::new(pool.clone());
    let deletion = tokio::spawn(async move {
        repo.advance_ordinary_deletion(owner, book, 1, "start", &token, hash)
            .await
    });
    wait_blocked_by(&pool, editor_pid).await;
    guard.commit().await.unwrap();

    tokio::time::timeout(Duration::from_secs(15), editor)
        .await
        .expect("chapter release must unblock author")
        .expect("author task must join")
        .expect("V2 must be published");
    let request_revision = tokio::time::timeout(Duration::from_secs(15), deletion)
        .await
        .expect("author commit must unblock deletion")
        .expect("deletion task must join")
        .expect("ordinary deletion request must succeed");
    assert_eq!(request_revision, 2);

    let (state, visibility): (String, String) =
        sqlx::query_as("SELECT lifecycle_status,visibility FROM books WHERE id=$1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), visibility.as_str()),
        ("pending_deletion", "draft")
    );
    let archived: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT chapter_id,version_id FROM book_layer_free_archive_versions
         WHERE book_id=$1 AND request_revision=$2",
    )
    .bind(book)
    .bind(request_revision)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(archived, vec![(chapter, version2)]);
    let saved: i64 = sqlx::query_scalar(
        "SELECT version_id FROM reader_saved_free_chapters WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(reader)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, v1.version_id);
    verify_receipt(&pool, chapter, v1.version_id, &initial.body).await;
    verify_receipt(&pool, chapter, version2, &expected_v2_body).await;

    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (2, 2, 2));
    assert_eq!(
        PgAuthoring::new(pool.clone())
            .advance_ordinary_deletion(owner, book, 1, "start", &token, hash)
            .await,
        Err(AuthorError::Conflict)
    );
    let archive_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_layer_free_archive_versions WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(archive_count, 1, "replay must not rewrite the archive");
    pool.close().await;
}
