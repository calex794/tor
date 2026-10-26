use sqlx::PgPool;
use tor_books::{
    repositories::PgCatalog,
    services::{CatalogError, GetPublicBook, ListPublicBooks, PublicBookRepository},
};
#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci and TEST_DATABASE_URL"]
async fn pg_visibility_and_readiness() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never mutate production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    assert_eq!(
        catalog.get_public(1).await.unwrap().unwrap().title(),
        "TOR CI public"
    );
    assert!(catalog.get_public(2).await.unwrap().is_none());
    assert!(catalog.get_public(3).await.unwrap().is_none());
    assert!(matches!(
        GetPublicBook::new(&catalog).execute(2).await,
        Ok(None)
    ));
    assert!(matches!(
        ListPublicBooks::new(&catalog).execute(101).await,
        Err(CatalogError::InvalidLimit)
    ));
    pool.close().await;
    // A cloned PgPool holds the same pool; closing one handle is not enough.
    // Close the pool owned by the repository via a test-only helper below.
    catalog.close_for_test().await;
    assert!(catalog.probe().await.is_err());
    assert!(matches!(
        catalog.list_public(20).await,
        Err(CatalogError::Unavailable)
    ));
}

#[tokio::test]
#[ignore = "requires seeded isolated tor_ci with G-09 book and user 905"]
async fn b_public_cover_variant_revalidates_current_visibility() {
    use tor_books::repositories::cover_slots::{PgCoverSlots, PublicCoverKind};

    let url = std::env::var("TEST_DATABASE_URL").expect("isolated tor_ci");
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never mutate production");

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE books SET visibility='public',admin_hold=FALSE,is_deleted=FALSE WHERE id=905",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let source: i64 = sqlx::query_scalar(
        "INSERT INTO cover_upload_objects
           (book_id,uploader_user_id,storage_key,content_sha256,content_type,size_bytes,status)
         VALUES (905,905,'staging/covers/ci-public-projection-905',
                 decode(repeat('11',32),'hex'),'image/png',100,'verified')
         RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO cover_variant_jobs
           (source_object_id,book_id,actor_user_id,recipe_version,expected_slot_revision,status)
         VALUES ($1,905,905,'public-read-01',0,'done')
         RETURNING id",
    )
    .bind(source)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cover_variants
           (job_id,variant_name,storage_key,content_sha256,size_bytes)
         VALUES ($1,'card','staging/covers/variants/ci-read-905-card.webp',
                 decode(repeat('22',32),'hex'),101),
                ($1,'main','staging/covers/variants/ci-read-905-main.webp',
                 decode(repeat('33',32),'hex'),202)",
    )
    .bind(job)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO book_cover_slots(book_id,job_id) VALUES (905,$1)")
        .bind(job)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let covers = PgCoverSlots::new(pool.clone());
    let card = covers
        .public_variant(905, PublicCoverKind::Card)
        .await
        .unwrap()
        .expect("public card");
    assert_eq!(card.sha256, [0x22; 32]);
    assert_eq!(card.size_bytes, 101);
    assert_eq!(card.revision, 1);
    let main = covers
        .public_variant(905, PublicCoverKind::Main)
        .await
        .unwrap()
        .expect("public main");
    assert_eq!(main.sha256, [0x33; 32]);
    assert_eq!(main.size_bytes, 202);
    assert!(covers
        .public_variant(906, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=905")
        .execute(&pool)
        .await
        .unwrap();
    assert!(covers
        .public_variant(905, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=905")
        .execute(&pool)
        .await
        .unwrap();
    assert!(covers
        .public_variant(905, PublicCoverKind::Main)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE books SET visibility='public',is_deleted=TRUE WHERE id=905")
        .execute(&pool)
        .await
        .unwrap();
    assert!(covers
        .public_variant(905, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    pool.close().await;
}
