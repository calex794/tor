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
    let batch = covers.public_card_ids(&[1, 2, 3, 905, 906]).await.unwrap();
    assert_eq!(batch.len(), 1);
    assert!(batch.contains(&905));
    assert!(covers.public_card_ids(&[]).await.unwrap().is_empty());
    assert!(covers.public_card_ids(&[905; 101]).await.is_err());
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
    assert!(covers.public_card_ids(&[905]).await.unwrap().is_empty());
    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=905")
        .execute(&pool)
        .await
        .unwrap();
    assert!(covers
        .public_variant(905, PublicCoverKind::Main)
        .await
        .unwrap()
        .is_none());
    assert!(covers.public_card_ids(&[905]).await.unwrap().is_empty());
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

#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci, seeded fixtures, and migration 202610280007"]
async fn shadow_manual_opt_in_is_not_automatic_and_views_are_read_only() {
    use sqlx::Row;
    use tor_books::repositories::PgShadowCatalog;

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never mutate production");

    // No side effects outside the transaction: the catalog remains empty by default.
    let mut tx = pool.begin().await.unwrap();
    let old_state: (bool, String) =
        sqlx::query_as("SELECT shadow_enabled, seo_index_owner FROM books WHERE id=1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(old_state, (false, "pandaolate".to_owned()));

    let initially_visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM shadow_public_books WHERE id=1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(initially_visible, 0);

    sqlx::query("UPDATE books SET shadow_enabled=TRUE WHERE id=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM shadow_public_books WHERE id=1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 1);
    let owner: String =
        sqlx::query_scalar("SELECT seo_index_owner FROM shadow_public_books WHERE id=1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(owner, "pandaolate", "manual opt-in must not change SEO");

    // The future, draft, moderated and blocked chapters are not readable even
    // if their parent has been allowed into Shadow. Paid metadata is not a veto.
    let rows = sqlx::query("SELECT id FROM shadow_public_chapters WHERE book_id=1 ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let ids: Vec<i64> = rows.iter().map(|row| row.get("id")).collect();
    assert_eq!(ids, vec![101, 103, 108]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name='shadow_public_chapters' AND column_name='body_markdown'",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap(),
        0,
        "Shadow metadata view must never expose chapter text"
    );

    // The main app may SELECT, but cannot modify the public projections.
    let can_insert: bool = sqlx::query_scalar(
        "SELECT has_table_privilege(current_user, 'shadow_public_books', 'INSERT')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let can_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege(current_user, 'shadow_public_books', 'UPDATE')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!can_insert && !can_update);
    tx.rollback().await.unwrap();

    // The Shadow adapter must not accept a main-site DB credential.
    let result = PgShadowCatalog::connect(&url).await;
    assert!(result.is_err(), "tor_app credentials must be rejected");
    pool.close().await;
}
