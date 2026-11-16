//! S001/R1: unavailable PostgreSQL must be an error, never an empty or leaked catalogue.
use super::*;

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_closed_pool_search_fails_closed_and_recovery_keeps_drafts_hidden() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let witness = PgPool::connect(&url).await.unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let term = format!("r1outage{stamp}");
    let visible = format!("{term} Visible");
    let secret = format!("{term} Hidden");
    let source_public = format!("ci:s001:outage:{stamp}:public");
    let source_draft = format!("ci:s001:outage:{stamp}:draft");
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru',$2,'public','basic'),($3,'ru',$4,'draft','basic')",
    )
    .bind(&source_public)
    .bind(&visible)
    .bind(&source_draft)
    .bind(&secret)
    .execute(&witness)
    .await
    .unwrap();

    let failing_pool = PgPool::connect(&url).await.unwrap();
    let failing_search = PgSearch::new(failing_pool.clone());
    let before = failing_search.search(&term, None, 20).await.unwrap();
    assert!(before.items.iter().any(|book| book.title == visible));
    assert!(!before.items.iter().any(|book| book.title == secret));

    // Simulate a dependency outage without killing PostgreSQL shared by CI.
    failing_pool.close().await;
    assert!(
        matches!(
            failing_search.search(&term, None, 20).await,
            Err(SearchError::Unavailable)
        ),
        "dependency failures must not be reported as a valid empty search"
    );

    let recovered = PgSearch::new(witness.clone());
    let after = recovered.search(&term, None, 20).await.unwrap();
    assert!(after.items.iter().any(|book| book.title == visible));
    assert!(!after.items.iter().any(|book| book.title == secret));
    assert_eq!(after.items.len(), 1);
    witness.close().await;
}
