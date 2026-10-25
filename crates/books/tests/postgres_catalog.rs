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
