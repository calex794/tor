//! DROP-07: reproduce reference races on the real schema in disposable PG18.
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

struct Edition {
    actor: i64,
    book: i64,
    chapter: i64,
    version: i64,
    source_pack: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct Evidence {
    generation: i64,
    saved: bool,
    archived: bool,
}

impl Edition {
    async fn seed(pool: &PgPool) -> Self {
        let mut tx = pool.begin().await.unwrap();
        let (actor, book, chapter, version) = sqlx::query_as(
            "WITH fixture AS (SELECT txid_current() AS id), account AS (
                INSERT INTO users(email_normalized,display_name,password_hash)
                SELECT 'drop07-race-'||id||'@example.invalid','DROP07 race','$argon2id$test'
                  FROM fixture RETURNING id
             ), book AS (
                INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
                SELECT 'test:drop07:race:'||fixture.id,'ru','DROP07 race','draft',account.id
                  FROM fixture,account RETURNING id,owner_user_id
             ), chapter AS (
                INSERT INTO chapters(book_id,chapter_no,title)
                SELECT id,1,'FREE edition' FROM book RETURNING id,book_id
             ), edition AS (
                INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
                SELECT id,1,'free',sha256('free'::bytea) FROM chapter RETURNING chapter_id,id
             )
             SELECT book.owner_user_id,book.id,edition.chapter_id,edition.id
             FROM book,edition",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();

        let mut source_pack = 0;
        for generation in [1_i64, 2] {
            let stem = format!("private/free-packs/ci/drop07-race-{book}-{generation}");
            let pack: i64 = sqlx::query_scalar(
                "INSERT INTO free_storage_packs(bucket,object_key,manifest_key,pack_sha256,
                    manifest_sha256,pack_bytes,manifest_bytes,seek_table_start)
                 VALUES('tor-ci-drop07-race',$1,$2,sha256($1::text::bytea),
                    sha256($2::text::bytea),100,30,50) RETURNING id",
            )
            .bind(format!("{stem}.zst"))
            .bind(format!("{stem}.manifest.json"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO free_edition_storage_generations(chapter_id,version_id,generation,
                    placement_kind,content_sha256,size_bytes,pack_id,frame_offset,frame_bytes)
                 VALUES($1,$2,$3,'pack',sha256('free'::bytea),4,$4,0,50)",
            )
            .bind(chapter)
            .bind(version)
            .bind(generation)
            .bind(pack)
            .execute(&mut *tx)
            .await
            .unwrap();
            if generation == 1 {
                source_pack = pack;
            }
        }

        let fixture = Self {
            actor,
            book,
            chapter,
            version,
            source_pack,
        };
        assert!(fixture.promote(&mut tx, 0, 1).await);
        tx.commit().await.unwrap();
        fixture
    }

    async fn evidence(&self, connection: &mut PgConnection) -> Evidence {
        let (generation, saved, archived) = sqlx::query_as(
            "SELECT generation,
                 EXISTS(SELECT 1 FROM reader_saved_free_chapters s
                        WHERE (s.chapter_id,s.version_id)=($1,$2)),
                 EXISTS(SELECT 1 FROM book_layer_free_archive_versions l
                        WHERE (l.chapter_id,l.version_id)=($1,$2))
               FROM free_edition_active_storage WHERE (chapter_id,version_id)=($1,$2)",
        )
        .bind(self.chapter)
        .bind(self.version)
        .fetch_one(connection)
        .await
        .unwrap();
        Evidence {
            generation,
            saved,
            archived,
        }
    }

    async fn add_saved(&self, connection: &mut PgConnection) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
             VALUES($1,$2,$3)",
        )
        .bind(self.actor)
        .bind(self.chapter)
        .bind(self.version)
        .execute(connection)
        .await?;
        Ok(())
    }

    async fn promote(&self, connection: &mut PgConnection, from: i64, to: i64) -> bool {
        sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,$3,$4)")
            .bind(self.chapter)
            .bind(self.version)
            .bind(from)
            .bind(to)
            .fetch_one(connection)
            .await
            .unwrap()
    }
}

async fn lock_evidence(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    // Lab only. NOWAIT rejects an already active writer. SHARE covers rows
    // that do not exist yet, and the active pointer needs an exclusive writer.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "LOCK TABLE book_layer_free_archive_versions, books, chapter_import_jobs,
             chapter_publications, chapters, free_edition_storage_generations,
             free_pack_activation_jobs, free_pack_direct_import_stages, free_pack_job_members,
             free_pack_jobs, free_storage_packs, reader_saved_free_chapters IN SHARE MODE NOWAIT",
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query("LOCK TABLE free_edition_active_storage IN SHARE ROW EXCLUSIVE MODE NOWAIT")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn assert_lock_unavailable(error: sqlx::Error) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03"),
        "{error}"
    );
}

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 /tor_ci"]
async fn drop_07k_pack_row_lock_misses_new_pins_bounded_fence_rechecks_before_cas() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let edition = Edition::seed(&pool).await;
    let mut observer = pool.acquire().await.unwrap();

    let mut stale = pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM free_storage_packs WHERE id=$1 FOR UPDATE")
        .bind(edition.source_pack)
        .execute(&mut *stale)
        .await
        .unwrap();
    let before = edition.evidence(&mut stale).await;
    assert_eq!(before.generation, 1);
    assert!(!before.saved && !before.archived);

    let mut archive = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *archive)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions(book_id,request_revision,chapter_id,version_id)
         VALUES($1,1,$2,$3)",
    )
    .bind(edition.book)
    .bind(edition.chapter)
    .bind(edition.version)
    .execute(&mut *archive)
    .await
    .unwrap();
    archive.commit().await.unwrap();
    assert_eq!(edition.evidence(&mut stale).await, before);
    assert!(edition.evidence(&mut observer).await.archived);
    stale.rollback().await.unwrap();

    let mut saved = pool.begin().await.unwrap();
    edition.add_saved(&mut saved).await.unwrap();
    let mut busy = pool.begin().await.unwrap();
    assert_lock_unavailable(lock_evidence(&mut busy).await.unwrap_err());
    busy.rollback().await.unwrap();
    saved.commit().await.unwrap();

    // Acquire the locks before the fresh SELECT, then discard stale evidence.
    let mut recheck = pool.begin().await.unwrap();
    lock_evidence(&mut recheck).await.unwrap();
    let fresh = edition.evidence(&mut recheck).await;
    assert_ne!(fresh, before);
    assert!(fresh.saved && fresh.archived);
    recheck.rollback().await.unwrap();
    assert_eq!(edition.evidence(&mut observer).await, fresh);

    // A matching retry moves this retained edition while the locks are held.
    let mut cutover = pool.begin().await.unwrap();
    lock_evidence(&mut cutover).await.unwrap();
    assert_eq!(edition.evidence(&mut cutover).await, fresh);
    let mut concurrent = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *concurrent)
        .await
        .unwrap();
    let blocked = sqlx::query("DELETE FROM reader_saved_free_chapters WHERE chapter_id=$1")
        .bind(edition.chapter)
        .execute(&mut *concurrent)
        .await
        .unwrap_err();
    assert_lock_unavailable(blocked);
    concurrent.rollback().await.unwrap();

    assert!(edition.promote(&mut cutover, 1, 2).await);
    assert_eq!(edition.evidence(&mut observer).await, fresh);
    cutover.commit().await.unwrap();
    assert_eq!(
        edition.evidence(&mut observer).await,
        Evidence {
            generation: 2,
            ..fresh
        }
    );
    let old_generation_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM free_edition_storage_generations
         WHERE (chapter_id,version_id,generation,pack_id)=($1,$2,1,$3))",
    )
    .bind(edition.chapter)
    .bind(edition.version)
    .bind(edition.source_pack)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(old_generation_exists);
}
