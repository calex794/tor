//! DROP-07: the real catalog cannot reuse a sealed source PACK.
use sqlx::{PgConnection, PgPool};
use tor_books::{
    repositories::{
        free_expected_inventory::{FreeInventoryError, PackInventoryRead, PgFreeExpectedInventory},
        free_pack_attach::PgFreePackCatalog,
        free_pack_jobs::PackJobClaim,
    },
    services::authoring::AuthorError,
};
use tor_contracts::drop_pack::{PackPlacementFrame, PackPlacementReceipt, PackPlacementTarget};

#[derive(Clone, Copy)]
struct Edition {
    chapter: i64,
    version: i64,
}

async fn seed(pool: &PgPool) -> (i64, i64, Vec<Edition>) {
    let mut tx = pool.begin().await.unwrap();
    let (actor, book) = sqlx::query_as(
        "WITH account AS (
            INSERT INTO users(email_normalized,display_name,password_hash)
            VALUES('drop07-gate-'||pg_current_xact_id()||'@example.invalid',
                   'DROP placement gate','$argon2id$test') RETURNING id
         ) INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         SELECT 'test:drop07:placement:'||id,'ru','DROP placement gate','draft',id FROM account
         RETURNING owner_user_id,id",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let chapters = sqlx::query_as::<_, (i64, i64)>(
        "WITH chapters AS (
            INSERT INTO chapters(book_id,chapter_no,title)
            VALUES ($1,1,'First edition'),($1,2,'Second edition') RETURNING id
         ) INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         SELECT id,1,'free',sha256('free'::bytea) FROM chapters RETURNING chapter_id,id",
    )
    .bind(book)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (
        actor,
        book,
        chapters
            .into_iter()
            .map(|(chapter, version)| Edition { chapter, version })
            .collect(),
    )
}

async fn register(pool: &PgPool, book: i64, bucket: &str, label: &str) -> i64 {
    let key = format!("private/free-packs/ci/drop07-gate-{book}-{label}");
    sqlx::query_scalar(
        "INSERT INTO free_storage_packs(bucket,object_key,manifest_key,pack_sha256,
            manifest_sha256,pack_bytes,manifest_bytes,seek_table_start)
         VALUES($1,$2,$3,sha256($2::text::bytea),sha256($3::text::bytea),150,30,120)
         RETURNING id",
    )
    .bind(bucket)
    .bind(format!("{key}.zst"))
    .bind(format!("{key}.manifest.json"))
    .fetch_one(pool)
    .await
    .unwrap()
}

impl Edition {
    async fn attach(
        self,
        pool: &PgPool,
        generation: i64,
        pack: i64,
        offset: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO free_edition_storage_generations(chapter_id,version_id,generation,
                placement_kind,content_sha256,size_bytes,pack_id,frame_offset,frame_bytes)
             VALUES($1,$2,$3,'pack',sha256('free'::bytea),4,$4,$5,20)",
        )
        .bind(self.chapter)
        .bind(self.version)
        .bind(generation)
        .bind(pack)
        .bind(offset)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn promote(
        self,
        connection: &mut PgConnection,
        from: i64,
        to: i64,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,$3,$4)")
            .bind(self.chapter)
            .bind(self.version)
            .bind(from)
            .bind(to)
            .fetch_one(connection)
            .await
    }
}

async fn seal(connection: &mut PgConnection, pack: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT seal_replaced_free_pack_placements($1)")
        .bind(pack)
        .fetch_one(connection)
        .await
}

fn assert_sqlstate(error: sqlx::Error, expected: &str) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some(expected),
        "{error}"
    );
}

#[tokio::test]
#[ignore = "requires separate app and migrator roles on disposable PostgreSQL18 tor_ci"]
async fn drop_07_pack_seal_preserves_future_pins_and_denies_reactivation() {
    let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
    let owner_url = std::env::var("MIGRATOR_URL").unwrap();
    for url in [&app_url, &owner_url] {
        assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    }
    let app = PgPool::connect(&app_url).await.unwrap();
    let owner_pool = PgPool::connect(&owner_url).await.unwrap();
    let mut owner = owner_pool.acquire().await.unwrap();
    let (actor, book, editions) = seed(&app).await;
    let [first, second] = editions.as_slice() else {
        panic!("expected two editions");
    };
    let bucket = "tor-ci-drop07-pack-gate";
    let source = register(&app, book, bucket, "source").await;
    let target = register(&app, book, bucket, "target").await;
    let wrong_bucket = register(&app, book, "tor-ci-drop07-other", "other").await;
    let mut writer = app.acquire().await.unwrap();
    assert_sqlstate(seal(&mut writer, source).await.unwrap_err(), "42501");
    assert_sqlstate(
        sqlx::query(
            "UPDATE free_pack_placement_gates SET sealed_at=clock_timestamp() WHERE pack_id=$1",
        )
        .bind(source)
        .execute(&mut *writer)
        .await
        .unwrap_err(),
        "42501",
    );
    assert!(!seal(&mut owner, source).await.unwrap()); // empty

    first.attach(&app, 1, source, 0).await.unwrap();
    second.attach(&app, 1, source, 20).await.unwrap();
    // Leave an already registered candidate with a higher generation in the
    // source. After sealing, monotonicity alone cannot prevent reactivation.
    first.attach(&app, 3, source, 40).await.unwrap();
    first.attach(&app, 2, target, 0).await.unwrap();
    second.attach(&app, 2, wrong_bucket, 0).await.unwrap();
    second.attach(&app, 3, target, 20).await.unwrap();
    assert!(first.promote(&mut writer, 0, 1).await.unwrap());
    assert!(second.promote(&mut writer, 0, 1).await.unwrap());
    assert!(first.promote(&mut writer, 1, 2).await.unwrap());
    assert!(!seal(&mut owner, source).await.unwrap()); // partial replacement
    assert!(second.promote(&mut writer, 1, 2).await.unwrap());
    assert!(!seal(&mut owner, source).await.unwrap()); // cross-bucket replacement
    assert!(second.promote(&mut writer, 2, 3).await.unwrap());

    let mut active_writer = app.begin().await.unwrap();
    assert!(first.promote(&mut active_writer, 2, 3).await.unwrap());
    assert_sqlstate(seal(&mut owner, source).await.unwrap_err(), "55P03");
    active_writer.rollback().await.unwrap();

    let mut stale_seal = owner_pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale_seal)
        .await
        .unwrap();
    assert_sqlstate(seal(&mut stale_seal, source).await.unwrap_err(), "25001");
    stale_seal.rollback().await.unwrap();

    let mut stale_writer = app.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale_writer)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM free_storage_packs WHERE id=$1")
        .bind(source)
        .execute(&mut *stale_writer)
        .await
        .unwrap();

    let (_, other_book, other_editions) = seed(&app).await;
    let other = other_editions[0];
    let other_pack = register(&app, other_book, bucket, "unrelated").await;
    other.attach(&app, 1, other_pack, 0).await.unwrap();
    let mut sealing = owner_pool.begin().await.unwrap();
    assert!(seal(&mut sealing, source).await.unwrap());
    let mut unrelated = app.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *unrelated)
        .await
        .unwrap();
    assert!(other.promote(&mut unrelated, 0, 1).await.unwrap());
    unrelated.commit().await.unwrap();
    sealing.commit().await.unwrap();
    assert!(seal(&mut owner, source).await.unwrap()); // exact retry
    assert_sqlstate(
        first.promote(&mut stale_writer, 2, 3).await.unwrap_err(),
        "40001",
    );
    stale_writer.rollback().await.unwrap();
    assert_sqlstate(first.promote(&mut writer, 2, 3).await.unwrap_err(), "23514");
    assert_sqlstate(
        first.attach(&app, 4, source, 60).await.unwrap_err(),
        "23514",
    );

    // Both writers still pin the exact old edition, and the active location
    // remains the same known replacement. The source receipt stays in PG.
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES ($1,$2,$3)",
    )
    .bind(actor)
    .bind(first.chapter)
    .bind(first.version)
    .execute(&mut *writer)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions(book_id,request_revision,chapter_id,version_id)
         VALUES ($1,1,$2,$3)",
    )
    .bind(book)
    .bind(first.chapter)
    .bind(first.version)
    .execute(&mut *writer)
    .await
    .unwrap();
    let active_and_source: (i64, bool) = sqlx::query_as(
        "SELECT g.pack_id,EXISTS(SELECT 1 FROM free_storage_packs WHERE id=$1)
         FROM reader_saved_free_chapters s
         JOIN book_layer_free_archive_versions l USING (chapter_id,version_id)
         JOIN free_edition_active_storage a USING (chapter_id,version_id)
         JOIN free_edition_storage_generations g USING (chapter_id,version_id,generation)
         WHERE s.user_id=$2 AND s.chapter_id=$3 AND s.version_id=$4",
    )
    .bind(source)
    .bind(actor)
    .bind(first.chapter)
    .bind(first.version)
    .fetch_one(&mut *writer)
    .await
    .unwrap();
    assert_eq!(active_and_source, (target, true));
}

#[tokio::test]
#[ignore = "requires separate app and migrator roles on disposable PostgreSQL18 tor_ci"]
async fn drop_07_background_reproof_hold_survives_terminal_queue_state() {
    let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
    let owner_url = std::env::var("MIGRATOR_URL").unwrap();
    for url in [&app_url, &owner_url] {
        assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    }
    let app = PgPool::connect(&app_url).await.unwrap();
    let owner_pool = PgPool::connect(&owner_url).await.unwrap();
    let mut owner = owner_pool.acquire().await.unwrap();
    let (actor, book, editions) = seed(&app).await;
    let edition = editions[0];
    let bucket = "tor-ci-drop07-pack-gate";
    let source = register(&app, book, bucket, "proof-source").await;
    let target = register(&app, book, bucket, "proof-target").await;
    edition.attach(&app, 1, source, 0).await.unwrap();
    edition.attach(&app, 2, target, 0).await.unwrap();
    let mut writer = app.acquire().await.unwrap();
    assert!(edition.promote(&mut writer, 0, 2).await.unwrap());

    let receipt = PackPlacementReceipt {
        target: PackPlacementTarget {
            bucket: bucket.to_owned(),
            pack_key: format!("private/free-packs/ci/drop07-gate-{book}-proof-source.zst"),
            manifest_key: format!(
                "private/free-packs/ci/drop07-gate-{book}-proof-source.manifest.json"
            ),
            pack_sha256: [1; 32],
            manifest_sha256: [2; 32],
            pack_bytes: 150,
            manifest_bytes: 30,
            seek_table_start: 120,
        },
        frames: vec![PackPlacementFrame {
            chapter_id: edition.chapter,
            version_id: edition.version,
            content_sha256: [3; 32],
            size_bytes: 4,
            frame_offset: 0,
            frame_bytes: 20,
        }],
    };
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO free_pack_jobs(actor_user_id,book_id,approved_fingerprint,
            expected_chapter_count,status,lease_epoch,pack_id)
         VALUES($1,$2,$3,1,'succeeded',1,$4) RETURNING id",
    )
    .bind(actor)
    .bind(book)
    .bind(PgFreePackCatalog::fingerprint(&receipt).as_slice())
    .bind(source)
    .fetch_one(&mut *writer)
    .await
    .unwrap();
    let claim = PackJobClaim {
        job_id: job,
        epoch: 1,
        actor_user_id: actor,
        book_id: book,
        receipt,
    };
    let catalog = PgFreePackCatalog::new(app.clone());

    let mut enqueue = app.begin().await.unwrap();
    sqlx::query("INSERT INTO free_pack_activation_jobs(pack_job_id) VALUES($1)")
        .bind(job)
        .execute(&mut *enqueue)
        .await
        .unwrap();
    assert_sqlstate(seal(&mut owner, source).await.unwrap_err(), "55P03");
    enqueue.commit().await.unwrap();
    assert_sqlstate(seal(&mut owner, source).await.unwrap_err(), "23514");
    sqlx::query(
        "UPDATE free_pack_activation_jobs SET status='running',lease_epoch=2,attempts=1,
           locked_until=clock_timestamp()+interval '1 minute' WHERE pack_job_id=$1",
    )
    .bind(job)
    .execute(&mut *writer)
    .await
    .unwrap();
    assert!(matches!(
        catalog
            .reserve_pack_activation_reproof(&claim, Some(1))
            .await,
        Err(AuthorError::Conflict)
    ));
    let mut forged = claim.clone();
    forged.receipt.target.pack_sha256 = [8; 32];
    assert!(matches!(
        catalog
            .reserve_pack_activation_reproof(&forged, Some(2))
            .await,
        Err(AuthorError::Conflict)
    ));
    catalog
        .reserve_pack_activation_reproof(&claim, Some(2))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE free_pack_activation_jobs SET status='failed',locked_until=NULL,
           completed_at=clock_timestamp() WHERE pack_job_id=$1",
    )
    .bind(job)
    .execute(&mut *writer)
    .await
    .unwrap();
    assert_sqlstate(seal(&mut owner, source).await.unwrap_err(), "23514");

    // Move the durable hold into the past on the disposable database.
    sqlx::query(
        "UPDATE free_pack_placement_gates SET reproof_until=clock_timestamp()-interval '1 second'
         WHERE pack_id=$1",
    )
    .bind(source)
    .execute(&mut *owner)
    .await
    .unwrap();
    assert!(seal(&mut owner, source).await.unwrap());
    assert!(matches!(
        catalog.reserve_pack_activation_reproof(&claim, None).await,
        Err(AuthorError::Conflict)
    ));
    assert_sqlstate(sqlx::query(
        "UPDATE free_pack_activation_jobs SET status='pending',completed_at=NULL WHERE pack_job_id=$1",
    ).bind(job).execute(&mut *writer).await.unwrap_err(), "23514");
}

#[tokio::test]
#[ignore = "requires separate app and migrator roles on disposable PostgreSQL18 tor_ci"]
async fn drop_07_inventory_read_reserves_exact_pack_pair_and_seal_refuses_it() {
    let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
    let owner_url = std::env::var("MIGRATOR_URL").unwrap();
    for url in [&app_url, &owner_url] {
        assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    }
    let app = PgPool::connect(&app_url).await.unwrap();
    let owner_pool = PgPool::connect(&owner_url).await.unwrap();
    let mut owner = owner_pool.acquire().await.unwrap();
    let (_, book, editions) = seed(&app).await;
    let edition = editions[0];
    let bucket = "tor-ci-drop07-inventory-gate";
    let source = register(&app, book, bucket, "inventory-source").await;
    let target = register(&app, book, bucket, "inventory-target").await;
    edition.attach(&app, 1, source, 0).await.unwrap();
    edition.attach(&app, 2, target, 0).await.unwrap();
    assert!(edition
        .promote(&mut app.acquire().await.unwrap(), 0, 2)
        .await
        .unwrap());

    let inventory = PgFreeExpectedInventory::new(app);
    let page = inventory.expected_page(bucket, None, 8).await.unwrap();
    let keys: Vec<_> = page
        .expected
        .iter()
        .filter(|entry| entry.object_key.contains("inventory-source"))
        .map(|entry| entry.object_key.as_str())
        .collect();
    assert_eq!(keys.len(), 2);
    assert_eq!(
        inventory
            .reserve_pack_read("tor-ci-wrong-bucket", keys[0])
            .await,
        Err(FreeInventoryError::Unavailable)
    );
    assert_eq!(
        inventory.reserve_pack_read(bucket, keys[0]).await,
        Ok(PackInventoryRead::Admitted)
    );
    assert_eq!(
        inventory.reserve_pack_read(bucket, keys[1]).await,
        Ok(PackInventoryRead::Admitted)
    );
    assert_sqlstate(seal(&mut owner, source).await.unwrap_err(), "23514");
    sqlx::query("UPDATE free_pack_placement_gates SET reproof_until=clock_timestamp()-interval '1 second' WHERE pack_id=$1")
        .bind(source).execute(&mut *owner).await.unwrap();
    assert!(seal(&mut owner, source).await.unwrap());
    // The original page still contains both expected receipts. Admission is
    // checked again at the actual read boundary, for both keys.
    for key in keys {
        assert_eq!(
            inventory.reserve_pack_read(bucket, key).await,
            Ok(PackInventoryRead::Sealed)
        );
    }
    assert_eq!(
        inventory.expected_page(bucket, None, 8).await.unwrap(),
        page
    );
}
