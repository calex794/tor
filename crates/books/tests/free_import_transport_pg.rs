//! Real PostgreSQL proof of per-job storage choice and mixed-checkpoint denial.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::{
    repositories::free_s3_publish::{
        FreeImportTransport, ImportTransportRequest, PgFreeS3Publication, VerifiedFreeImportEdition,
    },
    services::{
        authoring::AuthorError,
        chapter_file_import::{fingerprint, parse_text, ImportedChapter},
    },
};

struct Job {
    actor: i64,
    book: i64,
    id: i64,
    key: String,
    chapters: Vec<ImportedChapter>,
}
impl Job {
    async fn new(pool: &PgPool, label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let actor=sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Transport fixture','$argon2id$test') RETURNING id")
            .bind(format!("transport-{label}-{nonce}@tor.example")).fetch_one(pool).await.unwrap();
        let book=sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) VALUES($1,'ru','Transport fixture','draft',$2) RETURNING id")
            .bind(format!("ci:transport:{label}:{nonce}")).bind(actor).fetch_one(pool).await.unwrap();
        let key = "t".repeat(43);
        let source = "Глава 1: Проверка\nНеизменяемый исходник 😀";
        let chapters = parse_text("chapters.txt", source).unwrap();
        let digest = fingerprint(book, &chapters);
        let hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
        let id=sqlx::query_scalar("INSERT INTO chapter_import_jobs
            (book_id,actor_user_id,command_key,source_key,source_filename,source_sha256,source_bytes,
             approved_fingerprint,expected_chapter_count,status,lease_epoch,locked_until)
            VALUES($1,$2,$3,$4,'chapters.txt',$5,$6,$7,1,'running',1,clock_timestamp()+interval '120 seconds') RETURNING id")
            .bind(book).bind(actor).bind(&key).bind(format!("staging/chapters/{actor}/{book}/{key}"))
            .bind(hash.as_slice()).bind(source.len() as i64).bind(digest.as_slice()).fetch_one(pool).await.unwrap();
        Self {
            actor,
            book,
            id,
            key,
            chapters,
        }
    }
    fn request(&self, epoch: i64, prefer_pack: bool) -> ImportTransportRequest<'_> {
        ImportTransportRequest {
            job: (self.id, epoch),
            actor: self.actor,
            book: self.book,
            command_key: &self.key,
            chapters: &self.chapters,
            prefer_pack,
        }
    }
    async fn cancel(&self, pool: &PgPool) {
        sqlx::query("UPDATE chapter_import_jobs SET status='cancelled',locked_until=NULL,lease_epoch=lease_epoch+1 WHERE id=$1")
            .bind(self.id).execute(pool).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "disposable PG18 /tor_test only"]
async fn drop_04d_transport_is_immutable_fenced_and_adopts_legacy_single() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_test"));
    let pool = PgPool::connect(&url).await.unwrap();
    let repo = PgFreeS3Publication::new(pool.clone());
    let racing = Job::new(&pool, "racing").await;
    let (a, b) = tokio::join!(
        repo.select_import_transport(racing.request(1, true)),
        repo.select_import_transport(racing.request(1, false))
    );
    assert_eq!(
        a.unwrap(),
        b.unwrap(),
        "competing rollout settings must converge to one choice"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM free_chapter_import_transports WHERE job_id=$1")
            .bind(racing.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        repo.select_import_transport(racing.request(2, true)).await,
        Err(AuthorError::Conflict)
    );
    racing.cancel(&pool).await;

    let packed = Job::new(&pool, "packed").await;
    assert_eq!(
        repo.select_import_transport(packed.request(1, true))
            .await
            .unwrap(),
        FreeImportTransport::Pack
    );
    let pending = repo
        .prepare_import(packed.actor, packed.book, &packed.key, &packed.chapters)
        .await
        .unwrap();
    let p = &pending[0];
    let sha: [u8; 32] = Sha256::digest(packed.chapters[0].body.as_bytes()).into();
    let denied = repo
        .mark_import_verified(
            packed.actor,
            &packed.key,
            "tor-transport-ci",
            &VerifiedFreeImportEdition {
                chapter_id: p.chapter_id,
                version_id: p.version_id,
                sha256: sha,
                size_bytes: packed.chapters[0].body.len() as u64,
            },
        )
        .await;
    assert_eq!(
        denied,
        Err(AuthorError::Conflict),
        "PACK cannot record a SINGLE checkpoint"
    );
    sqlx::query("UPDATE chapter_import_jobs SET locked_until=clock_timestamp()-interval '1 second' WHERE id=$1").bind(packed.id).execute(&pool).await.unwrap();
    assert_eq!(
        repo.select_import_transport(packed.request(1, false)).await,
        Err(AuthorError::Conflict)
    );
    sqlx::query("UPDATE chapter_import_jobs SET lease_epoch=2,locked_until=clock_timestamp()+interval '120 seconds' WHERE id=$1").bind(packed.id).execute(&pool).await.unwrap();
    assert_eq!(
        repo.select_import_transport(packed.request(2, false))
            .await
            .unwrap(),
        FreeImportTransport::Pack,
        "rollout disabled after crash must preserve PACK"
    );
    packed.cancel(&pool).await;

    let legacy = Job::new(&pool, "legacy-single").await;
    let pending = repo
        .prepare_import(legacy.actor, legacy.book, &legacy.key, &legacy.chapters)
        .await
        .unwrap();
    // B fixture representing an already verified legacy G checkpoint. This
    // test proves B's decision; the separate MinIO worker test proves bytes.
    let p = &pending[0];
    let sha: [u8; 32] = Sha256::digest(legacy.chapters[0].body.as_bytes()).into();
    repo.mark_import_verified(
        legacy.actor,
        &legacy.key,
        "tor-transport-ci",
        &VerifiedFreeImportEdition {
            chapter_id: p.chapter_id,
            version_id: p.version_id,
            sha256: sha,
            size_bytes: legacy.chapters[0].body.len() as u64,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.select_import_transport(legacy.request(1, true))
            .await
            .unwrap(),
        FreeImportTransport::Single,
        "legacy checkpoints retain SINGLE despite rollout enabled"
    );
    let changed =
        sqlx::query("UPDATE free_chapter_import_transports SET transport='pack' WHERE job_id=$1")
            .bind(legacy.id)
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(
        changed.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    let mixed=sqlx::query("INSERT INTO free_pack_direct_import_stages
        (job_id,actor_user_id,book_id,approved_fingerprint,chapter_count,bucket,object_key,pack_sha256,manifest_sha256,receipt)
        VALUES($1,$2,$3,$4,1,'tor-transport-ci',$5,$6,$6,'{\"target\":{},\"frames\":[]}'::jsonb)")
        .bind(legacy.id).bind(legacy.actor).bind(legacy.book).bind(fingerprint(legacy.book,&legacy.chapters).as_slice())
        .bind(format!("private/free-packs/import/{}/{}.zst",legacy.book,"a".repeat(64))).bind([7u8;32].as_slice()).execute(&pool).await.unwrap_err();
    assert!(mixed
        .as_database_error()
        .unwrap()
        .message()
        .contains("SINGLE job cannot checkpoint PACK"));
    legacy.cancel(&pool).await;
    println!("DROP04D_TRANSPORT_PG_PASS: concurrent choice, lease fence, flag flip, legacy checkpoint and SQL mixed-mode denial");
    pool.close().await;
}
