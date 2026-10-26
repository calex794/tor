use sqlx::{postgres::PgPoolOptions, PgPool};
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
    // A temporary, connection-local SQLx history shadows the real history
    // without modifying shared CI tables. Old baseline 009 is NOT sufficient
    // to serve B-06 author requests from a new binary.
    let old_schema_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("CREATE TEMP TABLE _sqlx_migrations (version BIGINT, success BOOLEAN)")
        .execute(&old_schema_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO _sqlx_migrations (version,success) VALUES (202610130003,TRUE)")
        .execute(&old_schema_pool)
        .await
        .unwrap();
    let old_catalog = PgCatalog::new(old_schema_pool.clone());
    assert!(
        old_catalog.probe().await.is_err(),
        "new runtime must refuse readiness before B-06 schema migration"
    );
    old_schema_pool.close().await;

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
