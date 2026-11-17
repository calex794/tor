//! DROP-07a: disposable PG18 enumeration of protected FREE object receipts.
//! Never contacts a real provider or changes storage placement.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::{
        free_expected_inventory::{FreeObjectKind, PgFreeExpectedInventory},
        free_s3_publish::PgFreeS3Publication,
    },
    services::authoring::{save_chapter, SaveChapterRequest},
};

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
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().any(|(kind, _)| *kind == FreeObjectKind::Single));
    assert!(seen.iter().any(|(kind, _)| *kind == FreeObjectKind::Pack));
    assert!(seen.iter().any(|(kind, _)| *kind == FreeObjectKind::Manifest));
    let done = inventory.expected_page(&bucket, cursor.as_deref(), 1).await.unwrap();
    assert!(done.expected.is_empty());
    assert!(done.next_after_key.is_none());
    assert!(inventory.expected_page("public-data", None, 1).await.is_err());
    assert!(inventory.expected_page(&bucket, None, 129).await.is_err());
    // Both canonical registries remain intact: this is NEVER a GC job.
    let preserved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM free_chapter_storage_objects WHERE bucket=$1",
    )
    .bind(&bucket)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved, 1);
    println!("DROP07A_PG_READONLY_PASS: SINGLE + PACK + manifest, strict cursor, no mutation");
    pool.close().await;
}
