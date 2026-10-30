use sqlx::PgPool;
use tor_search::PgSearch;

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and exact projection migration"]
async fn f_02_public_projection_exact_on_bulk_visibility_and_rollback() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    let identity: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(identity, "tor_ci", "must never change live data");

    let can_modify: bool = sqlx::query_scalar(
        "SELECT has_table_privilege(current_user,'public_catalog_facets','UPDATE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !can_modify,
        "application role must not alter exact facet counts"
    );

    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,
                           admin_hold,is_deleted,genres,tags,fandoms)
         VALUES
         ('ci:perf233:count:a','ru','PERF233 A','public',false,false,
          ARRAY['perf233-exact','perf233-exact'],
          ARRAY['perf233-tag'],ARRAY['perf233-fandom']),
         ('ci:perf233:count:b','ru','PERF233 B','draft',false,false,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom']),
         ('ci:perf233:count:c','ru','PERF233 C','public',true,false,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom']),
         ('ci:perf233:count:d','ru','PERF233 D','public',false,true,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom'])",
    )
    .execute(&pool)
    .await
    .expect("batch seed");

    let search = PgSearch::new(pool.clone());
    assert_public_catalog_projection_count(&pool).await;
    let reference_count: i64 = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-exact'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        reference_count, 1,
        "duplicate genre inside book counts once"
    );
    assert!(search
        .taxonomy_options()
        .await
        .unwrap()
        .genres
        .contains(&"perf233-exact".to_owned()));

    // Unhide one book, then change its arrays and verify one-to-one deltas.
    sqlx::query(
        "UPDATE books SET visibility='public'
         WHERE source_identity='ci:perf233:count:b'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-exact'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 2);
    sqlx::query(
        "UPDATE books SET genres=ARRAY['perf233-new'],tags=ARRAY['perf233-new-tag']
         WHERE source_identity='ci:perf233:count:b'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-exact'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert_public_catalog_projection_count(&pool).await;

    let mut transaction = pool.begin().await.expect("transaction");
    sqlx::query(
        "UPDATE books SET visibility='draft'
         WHERE source_identity='ci:perf233:count:a'",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    let within: Option<i64> = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-exact'",
    )
    .fetch_optional(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(within, None, "facet disappears in the same transaction");
    transaction.rollback().await.unwrap();
    let restored: i64 = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-exact'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(restored, 1, "rollback must restore the public projection");

    sqlx::query(
        "UPDATE books SET admin_hold=true
         WHERE source_identity='ci:perf233:count:a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        !search
            .taxonomy_options()
            .await
            .unwrap()
            .genres
            .contains(&"perf233-exact".to_owned()),
        "revoked genre must disappear immediately"
    );
    assert_public_catalog_projection_count(&pool).await;

    let total: u64 = search
        .count_catalog(tor_search::repositories::CatalogFacets {
            term: "",
            language: None,
            genre: None,
            tag: None,
            fandom: None,
        })
        .await
        .unwrap();
    let database_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM books
         WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, database_total as u64);

    // A delete is a real SQL DELETE, not merely a soft-delete flag.
    sqlx::query("DELETE FROM books WHERE source_identity='ci:perf233:count:b'")
        .execute(&pool)
        .await
        .unwrap();
    let absent: Option<i64> = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name='perf233-new'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(absent, None);
    assert_public_catalog_projection_count(&pool).await;
}

async fn assert_public_catalog_projection_count(pool: &PgPool) {
    // One MVCC snapshot: no false discrepancy from concurrent test writes.
    let (stored, actual): (i64, i64) = sqlx::query_as(
        "SELECT s.visible_book_count,
                (SELECT COUNT(*) FROM books
                 WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted)
         FROM public_catalog_statistics s WHERE singleton",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        stored, actual,
        "transactional total must match source books"
    );
}
