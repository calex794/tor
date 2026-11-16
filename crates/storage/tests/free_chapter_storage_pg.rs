//! S349: additive free S3 receipt schema; no existing reader or author path changes.
#![cfg(feature = "pg")]
use sqlx::{PgPool, Row};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn s349_free_storage_reference_is_edition_exact_immutable_and_private() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG18 CI database");
    assert!(url.ends_with("/tor_ci"), "never touch a live database");
    let pool = PgPool::connect(&url).await.unwrap();

    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s349-owner@example.invalid','S349 owner','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s349:metadata','ru','S349 private file','draft',$1)
         RETURNING id",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let free_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free chapter','draft','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Paid chapter','draft','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let text = "Неизменяемая бесплатная глава";
    let digest: Vec<u8> = sqlx::query_scalar("SELECT sha256(convert_to($1,'UTF8'))")
        .bind(text)
        .fetch_one(&pool)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(free_chapter)
    .bind(text)
    .bind(&digest)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,'paid',sha256(convert_to('paid','UTF8'))) RETURNING id",
    )
    .bind(paid_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    let key = format!("private/free-chapters/{free_chapter}/{version}.md");
    let saved: i64 = sqlx::query_scalar(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,$5)
         RETURNING version_id",
    )
    .bind(free_chapter)
    .bind(version)
    .bind(&key)
    .bind(&digest)
    .bind(text.len() as i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, version);
    let meta = sqlx::query(
        "SELECT object_key,content_type,sha256 FROM free_chapter_storage_objects
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(meta.get::<String, _>("object_key"), key);
    assert_eq!(
        meta.get::<String, _>("content_type"),
        "text/plain; charset=utf-8"
    );
    assert_eq!(meta.get::<Vec<u8>, _>("sha256"), digest);

    // Hash and chapter access are authoritative; a receipt cannot cross
    // from a paid version, or masquerade as an arbitrary S3 object.
    let forged_paid = sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,10)",
    )
    .bind(paid_chapter)
    .bind(paid_version)
    .bind(format!(
        "private/free-chapters/{paid_chapter}/{paid_version}.md"
    ))
    .bind(&digest)
    .execute(&pool)
    .await;
    assert!(forged_paid.is_err());
    let wrong_hash = sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,10)",
    )
    .bind(free_chapter)
    .bind(version)
    .bind(&key)
    .bind(vec![1_u8; 32])
    .execute(&pool)
    .await;
    assert!(wrong_hash.is_err());
    assert!(sqlx::query(
        "UPDATE free_chapter_storage_objects SET size_bytes=15
             WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM free_chapter_storage_objects
             WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .execute(&pool)
    .await
    .is_err());
    pool.close().await;
}

/// DROP-R2: the new physical-placement catalog is additive and inert. It must
/// not change the existing S349 SINGLE receipt or live reader publication.
/// All data here are synthetic and confined to the disposable tor_ci PG18 DB.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn drop_r2_pack_placement_generations_are_immutable_and_cas_fenced() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG18 CI database");
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"),
        "never touch an operator database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('drop-r2-owner@example.invalid','DROP R2','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:drop:r2:pack','ru','DROP R2 storage','draft',$1)
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free single V1','draft','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Paid should be excluded','draft','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let text = "DROP-R2: неизменяемая бесплатная V1";
    let digest: Vec<u8> = sqlx::query_scalar("SELECT sha256(convert_to($1,'UTF8'))")
        .bind(text)
        .fetch_one(&pool)
        .await
        .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(text)
    .bind(&digest)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,'Paid V1',sha256(convert_to('Paid V1','UTF8'))) RETURNING id",
    )
    .bind(paid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let original_key = format!("private/free-chapters/{chapter}/{v1}.md");
    sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,$5)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(&original_key)
    .bind(&digest)
    .bind(text.len() as i64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .unwrap();

    let pack: i64 = sqlx::query_scalar(
        "INSERT INTO free_storage_packs
         (bucket,object_key,manifest_key,pack_sha256,manifest_sha256,
          manifest_bytes,pack_bytes,seek_table_start)
         VALUES('tor-private-free-ci',
                'private/free-packs/ci/drop-r2-first.zst',
                'private/free-packs/ci/drop-r2-first.manifest.json',
                $1,$2,4096,16000,15000)
         RETURNING id",
    )
    .bind(vec![17_u8; 32])
    .bind(vec![19_u8; 32])
    .fetch_one(&pool)
    .await
    .unwrap();

    // Only a real preexisting S349 exact-edition SINGLE receipt may supply
    // a single placement generation; forged digest or size is denied.
    sqlx::query(
        "INSERT INTO free_edition_storage_generations
         (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes)
         VALUES($1,$2,1,'single',$3,$4)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(&digest)
    .bind(text.len() as i64)
    .execute(&pool)
    .await
    .unwrap();
    assert!(sqlx::query(
        "INSERT INTO free_edition_storage_generations
             (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes)
             VALUES($1,$2,20,'single',$3,$4)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(&digest)
    .bind(text.len() as i64 + 1)
    .execute(&pool)
    .await
    .is_err());

    // A PACK generation is separate and has a bounded frame. This is only
    // metadata: the future G gateway still needs real Range/sha verification.
    sqlx::query(
        "INSERT INTO free_edition_storage_generations
         (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes,
          pack_id,frame_offset,frame_bytes)
         VALUES($1,$2,2,'pack',$3,$4,$5,128,900)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(&digest)
    .bind(text.len() as i64)
    .bind(pack)
    .execute(&pool)
    .await
    .unwrap();
    for (generation, offset, size) in [(21_i64, 14_900_i64, 900_i64), (22, 128, 900)] {
        assert!(
            sqlx::query(
                "INSERT INTO free_edition_storage_generations
                 (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes,
                  pack_id,frame_offset,frame_bytes)
                 VALUES($1,$2,$3,'pack',$4,$5,$6,$7,$8)",
            )
            .bind(chapter)
            .bind(v1)
            .bind(generation)
            .bind(&digest)
            .bind(text.len() as i64)
            .bind(pack)
            .bind(offset)
            .bind(size)
            .execute(&pool)
            .await
            .is_err(),
            "frame must not exceed seek-table or overlap"
        );
    }
    let paid_digest: Vec<u8> =
        sqlx::query_scalar("SELECT content_sha256 FROM chapter_versions WHERE id=$1")
            .bind(paid_version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        sqlx::query(
            "INSERT INTO free_edition_storage_generations
             (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes,
              pack_id,frame_offset,frame_bytes)
             VALUES($1,$2,1,'pack',$3,7,$4,2000,100)",
        )
        .bind(paid)
        .bind(paid_version)
        .bind(&paid_digest)
        .bind(pack)
        .execute(&pool)
        .await
        .is_err(),
        "a paid edition cannot be smuggled into free PACK metadata"
    );
    assert!(
        sqlx::query("UPDATE free_storage_packs SET seek_table_start=14000 WHERE id=$1")
            .bind(pack)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "DELETE FROM free_edition_storage_generations
             WHERE chapter_id=$1 AND version_id=$2 AND generation=1",
    )
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .is_err());

    async fn promote(pool: &PgPool, chapter: i64, version: i64, before: i64, after: i64) -> bool {
        sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,$3,$4)")
            .bind(chapter)
            .bind(version)
            .bind(before)
            .bind(after)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    assert!(!promote(&pool, chapter, v1, 0, 999).await);
    assert!(promote(&pool, chapter, v1, 0, 1).await);
    assert!(!promote(&pool, chapter, v1, 0, 1).await);
    assert!(promote(&pool, chapter, v1, 1, 2).await);
    assert!(!promote(&pool, chapter, v1, 1, 2).await);
    assert!(
        sqlx::query(
            "UPDATE free_edition_active_storage
                     SET generation=1 WHERE chapter_id=$1 AND version_id=$2"
        )
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .is_err(),
        "active generation must never downgrade"
    );

    // Two real concurrent PostgreSQL transactions may race, but only one
    // can win an expected-generation CAS. Both candidate packs remain inert.
    for (generation, key) in [
        (3_i64, "private/free-packs/ci/drop-r2-candidate-three.zst"),
        (4_i64, "private/free-packs/ci/drop-r2-candidate-four.zst"),
    ] {
        let manifest_key = key.replace(".zst", ".manifest.json");
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO free_storage_packs
             (bucket,object_key,manifest_key,pack_sha256,manifest_sha256,
              manifest_bytes,pack_bytes,seek_table_start)
             VALUES('tor-private-free-ci',$1,$2,$3,$4,4096,16000,15000)
             RETURNING id",
        )
        .bind(key)
        .bind(&manifest_key)
        .bind(vec![generation as u8; 32])
        .bind(vec![(generation + 10) as u8; 32])
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO free_edition_storage_generations
             (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes,
              pack_id,frame_offset,frame_bytes)
             VALUES($1,$2,$3,'pack',$4,$5,$6,100,700)",
        )
        .bind(chapter)
        .bind(v1)
        .bind(generation)
        .bind(&digest)
        .bind(text.len() as i64)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let (left, right) = tokio::join!(
        promote(&pool, chapter, v1, 2, 3),
        promote(&pool, chapter, v1, 2, 4)
    );
    assert_ne!(
        left, right,
        "a concurrent generation switch must have one winner"
    );
    let current: i64 = sqlx::query_scalar(
        "SELECT generation FROM free_edition_active_storage
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((3..=4).contains(&current));

    // The physical switch is still inert: S349 publication and the original
    // separately purchased/saved FREE V1 object identity are unchanged.
    let published: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(published, v1);
    let original: String = sqlx::query_scalar(
        "SELECT object_key FROM free_chapter_storage_objects
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(original, original_key);
    assert!(sqlx::query(
        "DELETE FROM free_edition_active_storage
                     WHERE chapter_id=$1 AND version_id=$2"
    )
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .is_err());
    // DROP-R4a: separate promotion lease from the immutable candidate.
    // This does not claim that synthetic S3 bytes have been verified: that
    // proof is supplied only by the future R3b G/B integration.
    let pack_sha = vec![5_u8; 32];
    let manifest_sha = vec![15_u8; 32];
    let fingerprint = vec![42_u8; 32];
    let next_pack: i64 = sqlx::query_scalar(
        "INSERT INTO free_storage_packs
         (bucket,object_key,manifest_key,pack_sha256,manifest_sha256,
          manifest_bytes,pack_bytes,seek_table_start)
         VALUES('tor-private-free-ci',
         'private/free-packs/ci/drop-r4a-candidate-five.zst',
         'private/free-packs/ci/drop-r4a-candidate-five.manifest.json',
         $1,$2,4096,16000,15000) RETURNING id",
    )
    .bind(&pack_sha)
    .bind(&manifest_sha)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO free_edition_storage_generations
         (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes,
          pack_id,frame_offset,frame_bytes)
         VALUES($1,$2,5,'pack',$3,$4,$5,100,700)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(&digest)
    .bind(text.len() as i64)
    .bind(next_pack)
    .execute(&pool)
    .await
    .unwrap();
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO free_pack_promotion_jobs(
          chapter_id,version_id,expected_generation,next_generation,pack_id,
          expected_content_sha256,expected_size_bytes,expected_pack_sha256,
          expected_manifest_sha256,approved_fingerprint,lease_epoch,locked_until)
         VALUES($1,$2,$3,5,$4,$5,$6,$7,$8,$9,8,now()+interval '1 hour')
         RETURNING id",
    )
    .bind(chapter)
    .bind(v1)
    .bind(current)
    .bind(next_pack)
    .bind(&digest)
    .bind(text.len() as i64)
    .bind(&pack_sha)
    .bind(&manifest_sha)
    .bind(&fingerprint)
    .fetch_one(&pool)
    .await
    .unwrap();

    async fn finish(
        pool: &PgPool,
        job: i64,
        epoch: i64,
        fingerprint: &[u8],
        pack_sha: &[u8],
        manifest_sha: &[u8],
    ) -> bool {
        sqlx::query_scalar("SELECT finish_free_pack_promotion($1,$2,$3,$4,$5)")
            .bind(job)
            .bind(epoch)
            .bind(fingerprint)
            .bind(pack_sha)
            .bind(manifest_sha)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    assert!(!finish(&pool, job, 7, &fingerprint, &pack_sha, &manifest_sha).await);
    assert!(!finish(&pool, job, 8, &[31_u8; 32], &pack_sha, &manifest_sha).await);
    assert!(!finish(&pool, job, 8, &fingerprint, &[31_u8; 32], &manifest_sha).await);
    assert!(!finish(&pool, job, 8, &fingerprint, &pack_sha, &[31_u8; 32]).await);

    // A live admin hold blocks a physical promotion, not only the reader.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!finish(&pool, job, 8, &fingerprint, &pack_sha, &manifest_sha).await);
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE free_pack_promotion_jobs SET locked_until=now()-interval '1 second' WHERE id=$1",
    )
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!finish(&pool, job, 8, &fingerprint, &pack_sha, &manifest_sha).await);
    sqlx::query("UPDATE free_pack_promotion_jobs SET lease_epoch=9,locked_until=now()+interval '1 hour' WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!finish(&pool, job, 8, &fingerprint, &pack_sha, &manifest_sha).await);

    let (left, right) = tokio::join!(
        finish(&pool, job, 9, &fingerprint, &pack_sha, &manifest_sha),
        finish(&pool, job, 9, &fingerprint, &pack_sha, &manifest_sha)
    );
    assert_ne!(left, right, "exactly one current lease finalizer may win");
    assert!(!finish(&pool, job, 9, &fingerprint, &pack_sha, &manifest_sha).await);
    let final_generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM free_edition_active_storage
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(final_generation, 5);
    let final_status: String =
        sqlx::query_scalar("SELECT status FROM free_pack_promotion_jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(final_status, "succeeded");
    assert!(
        sqlx::query("UPDATE free_pack_promotion_jobs SET pack_id=$2 WHERE id=$1")
            .bind(job)
            .bind(pack)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE free_pack_promotion_jobs SET status='running' WHERE id=$1")
            .bind(job)
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1"
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap(),
        v1,
        "physical promotion must not change the published B edition"
    );
    pool.close().await;
}