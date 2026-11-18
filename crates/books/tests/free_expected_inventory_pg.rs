//! DROP-07a: disposable PG18 enumeration of protected FREE object receipts.
//! Never contacts a real provider or changes storage placement.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::{
        free_expected_inventory::{
            FreeInventoryError, FreeObjectKind, InventoryNamespace, PgFreeExpectedInventory,
        },
        free_key_quarantine::{
            FreeQuarantineError, PgFreeKeyQuarantine, QuarantineBackup, QuarantineObservation,
            QuarantineOutcome,
        },
        free_observed_reconcile::{FreeReferenceState, PgFreeObservedClassifier},
        free_pack_attach::PgFreePackCatalog,
        free_s3_publish::PgFreeS3Publication,
    },
    services::authoring::{save_chapter, SaveChapterRequest},
};
use tor_contracts::drop_pack::{PackPlacementFrame, PackPlacementReceipt, PackPlacementTarget};

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 /tor_ci"]
async fn drop_07j_quarantine_protects_historical_single_without_a_storage_receipt() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'DROP07 retention','$argon2id$test') RETURNING id",
    )
    .bind(format!("drop07j-{token}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (chapter, historical, current): (i64, i64, i64) = sqlx::query_as(
        "WITH book AS (
             INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru','DROP07 retention','public',$2) RETURNING id
         ), chapter AS (
             INSERT INTO chapters(book_id,chapter_no,title,visibility)
             SELECT id,1,'Retained chapter','public' FROM book RETURNING id,book_id
         ), editions AS (
             INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             SELECT chapter.id,edition.version_no,edition.body
             FROM chapter CROSS JOIN (VALUES (1,'old text'),(2,'current text')) edition(version_no,body)
             RETURNING chapter_id,id,version_no
         )
         SELECT v1.chapter_id,v1.id,v2.id FROM editions v1 JOIN editions v2 USING(chapter_id)
         WHERE v1.version_no=1 AND v2.version_no=2",
    )
    .bind(format!("ci:drop07j:{token}"))
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(current)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(actor)
    .bind(chapter)
    .bind(historical)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions(book_id,request_revision,chapter_id,version_id)
         SELECT book_id,1,id,$2 FROM chapters WHERE id=$1",
    )
    .bind(chapter)
    .bind(historical)
    .execute(&pool)
    .await
    .unwrap();

    let historical_key = format!("private/free-chapters/{chapter}/{historical}.md");
    let current_key = format!("private/free-chapters/{chapter}/{current}.md");
    let unrelated_key = format!("private/free-chapters/{}/{historical}.md", i64::MAX);
    let bucket = format!("tor-drop07j-{chapter}");
    let backup_bucket = format!("tor-drop07j-backup-{chapter}");
    let observation = QuarantineObservation {
        bucket: &bucket,
        key: &historical_key,
        version: "fixture-object-version",
        etag: "\"fixture-etag\"",
        sha256: [19; 32],
        size: 8,
    };
    let repo = PgFreeKeyQuarantine::new(pool.clone());
    assert_eq!(
        repo.enter(&observation).await,
        Err(FreeQuarantineError::Protected)
    );
    assert_eq!(
        repo.enter(&QuarantineObservation {
            key: &current_key,
            ..observation
        })
        .await,
        Err(FreeQuarantineError::Protected)
    );
    assert_eq!(
        repo.enter(&QuarantineObservation {
            key: &unrelated_key,
            ..observation
        })
        .await,
        Ok(QuarantineOutcome::Recorded)
    );

    // An older operator could have recorded a fence and backup evidence
    // before the logical edition guard existed. No retry may trust them alone.
    sqlx::query(
        "INSERT INTO free_storage_key_fences
           (bucket,object_key,purpose,object_version,etag,sha256,size_bytes,recorded_at)
         VALUES($1,$2,'quarantine',$3,$4,$5,$6,clock_timestamp()-interval '25 hours')",
    )
    .bind(&bucket)
    .bind(&historical_key)
    .bind(observation.version)
    .bind(observation.etag)
    .bind(observation.sha256.as_slice())
    .bind(observation.size as i64)
    .execute(&pool)
    .await
    .unwrap();
    let backup = QuarantineBackup {
        bucket: &backup_bucket,
        key: &historical_key,
        version: "fixture-backup-version",
    };
    assert_eq!(
        repo.enter(&observation).await,
        Err(FreeQuarantineError::Protected)
    );
    assert_eq!(
        repo.recheck(&observation, &backup).await,
        Err(FreeQuarantineError::Protected)
    );
    sqlx::query(
        "INSERT INTO free_storage_quarantine_rechecks
           (bucket,object_key,backup_bucket,backup_key,backup_version)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(&bucket)
    .bind(&historical_key)
    .bind(backup.bucket)
    .bind(backup.key)
    .bind(backup.version)
    .execute(&pool)
    .await
    .unwrap();
    let release = repo
        .find_release(&bucket, &historical_key, observation.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        repo.request_retirement(&release).await,
        Err(FreeQuarantineError::Protected)
    );
}

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 /tor_ci"]
async fn drop_07a_registered_single_pack_and_manifest_are_paginated_read_only() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'DROP07 inventory','$argon2id$test') RETURNING id",
    )
    .bind(format!("drop07-{token}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','DROP07 expected','draft',$2) RETURNING id",
    )
    .bind(format!("ci:drop07:{token}"))
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    let content = "Only this immutable free edition ✓";
    let bucket = format!("tor-private-drop07-{}", book);
    let request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        expected_revision: 0,
        number: 1,
        title: "chapter",
        body: content,
        access: "free",
        price_amount: 0,
        command_key: &"u".repeat(43),
    })
    .unwrap();
    let author = PgFreeS3Publication::new(pool.clone());
    let edition = author.prepare_new(actor, &request).await.unwrap();
    let pending_author_key = format!(
        "private/free-chapters/{}/{}.md",
        edition.chapter_id, edition.version_id
    );
    let classifier = PgFreeObservedClassifier::new(pool.clone());
    let prepared = classifier
        .classify_page(&bucket, &[pending_author_key])
        .await
        .unwrap();
    assert_eq!(prepared[0].state, FreeReferenceState::ProtectedPending);
    let digest: [u8; 32] = Sha256::digest(content.as_bytes()).into();
    author
        .attach_verified_and_publish(
            actor,
            book,
            (edition.chapter_id, edition.version_id),
            (bucket.as_str(), digest, content.len() as u64),
        )
        .await
        .unwrap();
    let pack_key = format!("private/free-packs/ci/drop07-{book}-immutable.zst");
    let manifest_key = pack_key.replace(".zst", ".manifest.json");
    sqlx::query(
        "INSERT INTO free_storage_packs(
           bucket,object_key,pack_sha256,manifest_key,manifest_sha256,
           pack_bytes,manifest_bytes,seek_table_start
         ) VALUES($1,$2,$3,$4,$5,1000,500,900)",
    )
    .bind(&bucket)
    .bind(&pack_key)
    .bind([18_u8; 32].as_slice())
    .bind(&manifest_key)
    .bind([19_u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();

    let inventory = PgFreeExpectedInventory::new(pool.clone());
    let mut cursor = None;
    let mut seen = Vec::new();
    for _ in 0..3 {
        let page = inventory
            .expected_page(&bucket, cursor.as_deref(), 1)
            .await
            .unwrap();
        assert_eq!(page.expected.len(), 1);
        let entry = &page.expected[0];
        assert_eq!(entry.sha256.len(), 32);
        seen.push((entry.kind, entry.object_key.clone()));
        cursor = page.next_after_key;
    }
    // DROP-07b: A pending R4a job protects its destination PACK and sidecar
    // even before the worker uploads either object. Unknown is never orphan.
    let pending_key = format!("private/free-packs/ci/drop07-{book}-pending.zst");
    let pending_manifest = pending_key.replace(".zst", ".manifest.json");
    let planned = PackPlacementReceipt {
        target: PackPlacementTarget {
            bucket: bucket.clone(),
            pack_key: pending_key.clone(),
            manifest_key: pending_manifest.clone(),
            pack_sha256: [21; 32],
            manifest_sha256: [22; 32],
            pack_bytes: 1000,
            manifest_bytes: 512,
            seek_table_start: 900,
        },
        frames: vec![PackPlacementFrame {
            chapter_id: edition.chapter_id,
            version_id: edition.version_id,
            content_sha256: digest,
            size_bytes: content.len() as u64,
            frame_offset: 0,
            frame_bytes: 80,
        }],
    };
    let job_catalog = PgFreePackCatalog::new(pool.clone());
    job_catalog
        .schedule_pack_job(actor, book, &planned)
        .await
        .unwrap();

    let source_key = format!(
        "private/free-chapters/{}/{}.md",
        edition.chapter_id, edition.version_id
    );
    let only_single = inventory
        .expected_page_in_namespace(&bucket, InventoryNamespace::Single, None, 3)
        .await
        .unwrap();
    assert_eq!(only_single.expected.len(), 1);
    assert_eq!(only_single.expected[0].object_key, source_key);
    assert!(only_single.next_after_key.is_none());
    let only_pack = inventory
        .expected_page_in_namespace(&bucket, InventoryNamespace::Pack, None, 1)
        .await
        .unwrap();
    let only_pack_tail = inventory
        .expected_page_in_namespace(
            &bucket,
            InventoryNamespace::Pack,
            only_pack.next_after_key.as_deref(),
            3,
        )
        .await
        .unwrap();
    assert_eq!(only_pack.expected[0].object_key, manifest_key);
    assert_eq!(only_pack_tail.expected.len(), 1);
    assert_eq!(only_pack_tail.expected[0].object_key, pack_key);
    assert!(matches!(
        inventory
            .expected_page_in_namespace(&bucket, InventoryNamespace::Pack, Some(&source_key), 1)
            .await,
        Err(FreeInventoryError::Invalid)
    ));
    let unknown_key = format!("private/free-packs/ci/drop07-{book}-unregistered.zst");
    let observed = [
        source_key.clone(),
        pack_key.clone(),
        manifest_key.clone(),
        pending_key.clone(),
        pending_manifest.clone(),
        unknown_key.clone(),
    ];
    let observations = PgFreeObservedClassifier::new(pool.clone())
        .classify_page(&bucket, &observed)
        .await
        .unwrap();
    assert_eq!(observations.len(), observed.len());
    let state = |key: &str| {
        observations
            .iter()
            .find(|item| item.object_key == key)
            .map(|item| item.state)
    };
    for name in [&source_key, &pack_key, &manifest_key] {
        assert_eq!(state(name), Some(FreeReferenceState::Registered));
    }
    for name in [&pending_key, &pending_manifest] {
        assert_eq!(state(name), Some(FreeReferenceState::ProtectedPending));
    }
    assert_eq!(state(&unknown_key), Some(FreeReferenceState::Unknown));

    // An approved immutable transport can exist before any PACK stage or
    // object receipt. The whole canonical book prefix is conservative evidence.
    let source_fingerprint = [23_u8; 32];
    let import_key = "q".repeat(43);
    let direct_job: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_import_jobs
           (book_id,actor_user_id,command_key,source_key,source_filename,source_sha256,
            source_bytes,approved_fingerprint,expected_chapter_count,status,locked_until,lease_epoch)
         VALUES ($1,$2,$3,$4,'drop07.txt',$5,100,$5,1,'running',
                 clock_timestamp()+interval '5 minutes',1) RETURNING id",
    )
    .bind(book)
    .bind(actor)
    .bind(&import_key)
    .bind(format!("staging/chapters/{actor}/{book}/{import_key}"))
    .bind(source_fingerprint.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO free_chapter_import_transports
            (job_id,actor_user_id,book_id,command_key,approved_fingerprint,chapter_count,transport)
         VALUES ($1,$2,$3,$4,$5,1,'pack')",
    )
    .bind(direct_job)
    .bind(actor)
    .bind(book)
    .bind(&import_key)
    .bind(source_fingerprint.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let direct_stem = "a".repeat(64);
    let direct_key = format!("private/free-packs/import/{book}/{direct_stem}.zst");
    let direct_manifest = direct_key.replace(".zst", ".manifest.json");
    let similar = format!("private/free-packs/ci/{book}/{direct_stem}.zst");
    let evidence = classifier
        .classify_page(&bucket, &[direct_key, direct_manifest, similar])
        .await
        .unwrap();
    assert_eq!(
        evidence
            .iter()
            .filter(|item| item.state == FreeReferenceState::ProtectedPending)
            .count(),
        2
    );
    assert_eq!(
        evidence
            .iter()
            .filter(|item| item.state == FreeReferenceState::Unknown)
            .count(),
        1
    );

    assert_eq!(seen.len(), 3);
    assert!(seen.iter().any(|(kind, _)| *kind == FreeObjectKind::Single));
    assert!(seen.iter().any(|(kind, _)| *kind == FreeObjectKind::Pack));
    assert!(seen
        .iter()
        .any(|(kind, _)| *kind == FreeObjectKind::Manifest));
    let done = inventory
        .expected_page(&bucket, cursor.as_deref(), 1)
        .await
        .unwrap();
    assert!(done.expected.is_empty());
    assert!(done.next_after_key.is_none());
    assert!(inventory
        .expected_page("public-data", None, 1)
        .await
        .is_err());
    assert!(inventory.expected_page(&bucket, None, 129).await.is_err());
    // Both canonical registries remain intact: this is NEVER a GC job.
    let preserved: i64 =
        sqlx::query_scalar("SELECT count(*) FROM free_chapter_storage_objects WHERE bucket=$1")
            .bind(&bucket)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(preserved, 1);
    println!("DROP07A_PG_READONLY_PASS: SINGLE + PACK + manifest, strict cursor, no mutation");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn drop_07h_quarantine_and_publication_share_a_durable_key_fence() {
    use tor_books::repositories::free_key_quarantine::{
        FreeQuarantineError as Error, PgFreeKeyQuarantine, QuarantineBackup, QuarantineObservation,
        QuarantineOutcome as Outcome,
    };
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let mut writer = pool.begin().await.unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bucket = format!("tor-private-gc-test-{unique}");
    let key = "private/free-chapters/70001/100.md";
    let other_key = "private/free-chapters/70001/101.md";
    let packed_key = "private/free-packs/quarantine/orphan-sample-266.zst";
    let manifest_key = "private/free-packs/quarantine/orphan-sample-266.manifest.json";
    let sample = QuarantineObservation {
        bucket: &bucket,
        key,
        version: "synthetic-version-v1",
        etag: "\"synthetic-etag\"",
        sha256: [66; 32],
        size: 4096,
    };
    let audit = PgFreeKeyQuarantine::new(pool.clone());

    sqlx::query("SELECT reserve_free_storage_publication($1,$2)")
        .bind(&bucket)
        .bind(key)
        .execute(&mut *writer)
        .await
        .unwrap();
    let competing = audit.enter(&sample);
    tokio::pin!(competing);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(75), &mut competing)
            .await
            .is_err()
    );
    writer.commit().await.unwrap();
    assert_eq!(competing.await, Err(Error::Protected));

    let packed = QuarantineObservation {
        key: packed_key,
        ..sample
    };
    assert_eq!(audit.enter(&packed).await, Ok(Outcome::Recorded));
    assert_eq!(audit.enter(&packed).await, Ok(Outcome::AlreadyRecorded));
    assert_eq!(
        audit
            .enter(&QuarantineObservation {
                version: "synthetic-v2",
                ..packed
            })
            .await,
        Err(Error::Protected)
    );
    assert_eq!(
        audit
            .reserve_publication(&bucket, &[other_key, packed_key])
            .await,
        Err(Error::Protected)
    );
    let partial: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_storage_key_fences WHERE bucket=$1 AND object_key=$2",
    )
    .bind(&bucket)
    .bind(other_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(partial, 0, "multi-key write reservation is atomic");

    // Legacy receipt path cannot attach bytes even if the writer bypassed
    // the pre-PUT check. Reserving the sidecar must roll back too.
    let blocked = sqlx::query(
        "INSERT INTO free_storage_packs
         (bucket,object_key,manifest_key,pack_sha256,manifest_sha256,pack_bytes,manifest_bytes,seek_table_start)
         VALUES ($1,$2,$3,$4,$4,4096,512,64)",
    ).bind(&bucket).bind(packed_key).bind(manifest_key).bind(packed.sha256.as_slice())
        .execute(&pool).await.unwrap_err();
    assert_eq!(
        blocked.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    let untouched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_storage_key_fences WHERE bucket=$1 AND object_key=$2",
    )
    .bind(&bucket)
    .bind(manifest_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(untouched, 0);

    let backup = QuarantineBackup {
        bucket: "tor-independent-archive",
        key: "v1/snapshot",
        version: "external-version",
    };
    assert_eq!(audit.recheck(&packed, &backup).await, Err(Error::Protected));

    // Fast-forward by seeding an already aged fixture. The normal path uses
    // the database clock and cannot alter existing quarantine timestamps.
    sqlx::query(
        "INSERT INTO free_storage_key_fences VALUES
         ($1,$2,'quarantine',$3,$4,$5,$6,clock_timestamp() - interval '25 hours')",
    )
    .bind(&bucket)
    .bind(other_key)
    .bind(sample.version)
    .bind(sample.etag)
    .bind(sample.sha256.as_slice())
    .bind(sample.size as i64)
    .execute(&pool)
    .await
    .unwrap();
    let aged = QuarantineObservation {
        key: other_key,
        ..sample
    };
    assert_eq!(
        audit.reserve_publication(&bucket, &[other_key]).await,
        Err(Error::Protected)
    );
    assert_eq!(audit.recheck(&aged, &backup).await, Ok(Outcome::Recorded));
    assert_eq!(
        audit.recheck(&aged, &backup).await,
        Ok(Outcome::AlreadyRecorded)
    );
    assert_eq!(
        audit
            .recheck(
                &aged,
                &QuarantineBackup {
                    version: "replaced-archive",
                    ..backup
                }
            )
            .await,
        Err(Error::Protected)
    );
    assert!(sqlx::query(
        "UPDATE free_storage_key_fences SET recorded_at=clock_timestamp() WHERE bucket=$1",
    )
    .bind(&bucket)
    .execute(&pool)
    .await
    .is_err());
    pool.close().await;
}
