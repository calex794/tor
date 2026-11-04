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
                           admin_hold,is_deleted,genres,tags,fandoms,access_level)
         VALUES
         ('ci:perf233:count:a','ru','PERF233 A','public',false,false,
          ARRAY['perf233-exact','perf233-exact'],
          ARRAY['perf233-tag'],ARRAY['perf233-fandom'],'basic'),
         ('ci:perf233:count:b','ru','PERF233 B','draft',false,false,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom'],'basic'),
         ('ci:perf233:count:c','ru','PERF233 C','public',true,false,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom'],'basic'),
         ('ci:perf233:count:d','ru','PERF233 D','public',false,true,
          ARRAY['perf233-exact'],ARRAY['perf233-tag'],ARRAY['perf233-fandom'],'basic')",
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
         WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND lifecycle_status<>'pending_deletion' AND NOT admin_hold AND NOT is_deleted",
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
                 WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND lifecycle_status<>'pending_deletion' AND NOT admin_hold AND NOT is_deleted)
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

#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci with S020 public basic projection"]
async fn s020_anonymous_catalog_excludes_restricted_books_and_exact_counts() {
    use tor_search::{
        repositories::{showcases::PgShowcases, CatalogFacets},
        services::showcases::ShowcaseRepository,
    };

    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    assert!(url.ends_with("/tor_ci"), "disposable database only");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let term = format!("AgeCatalog{nonce}");
    let facet = format!("s020-age-genre-{nonce}");
    let before: i64 = sqlx::query_scalar(
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for (name, level) in [
        ("basic", Some("basic")),
        ("adult", Some("adult")),
        ("pro", Some("pro_adult")),
        ("unknown", None),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)
             VALUES($1,'ru',$2,'public',$3,ARRAY[$4::text]) RETURNING id",
        )
        .bind(format!("ci:s020:catalog:{nonce}:{name}"))
        .bind(format!("{term} {name}"))
        .bind(level)
        .bind(&facet)
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, before + 1, "non-basic must not affect public count");
    let refs: i64 = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name=$1",
    )
    .bind(&facet)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refs, 1, "only basic book contributes to the facet");

    let search = PgSearch::new(pool.clone());
    let filters = CatalogFacets {
        term: &term,
        language: Some("ru"),
        genre: Some(&facet),
        tag: None,
        fandom: None,
    };
    for sort in ["oldest", "newest", "title", "author"] {
        let page = search
            .search_catalog(filters, None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(
            page.items.iter().map(|book| book.id).collect::<Vec<_>>(),
            vec![ids[0] as u64],
            "{sort} must not leak restricted titles"
        );
        let before_page = search
            .browse_catalog_page(filters, None, None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(before_page.items.len(), 1, "{sort} forward window");
    }
    assert_eq!(search.count_catalog(filters).await.unwrap(), 1);
    let spotlight = PgShowcases::new(pool.clone())
        .latest_public(Some(&facet), 24)
        .await
        .unwrap();
    assert_eq!(
        spotlight.iter().map(|book| book.id).collect::<Vec<_>>(),
        vec![ids[0] as u64],
        "anonymous homepage must not leak restricted books"
    );

    // A moderation state change and a tier upgrade must each update all
    // anonymous projections in the same transaction as the source Book.
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), 0);
    sqlx::query("UPDATE books SET visibility_status='active' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), 1);
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), 0);
    let refs: Option<i64> = sqlx::query_scalar(
        "SELECT reference_count FROM public_catalog_facets
         WHERE kind='genre' AND name=$1",
    )
    .bind(&facet)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        refs, None,
        "revoked last basic facet disappears immediately"
    );

    sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
        .bind(&ids)
        .execute(&pool)
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar(
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, before);
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and S020 lifecycle projection migration"]
async fn s020_pending_deletion_is_excluded_from_cached_count_and_facets_atomically() {
    use tor_search::repositories::CatalogFacets;
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated database");
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let identity: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(identity, "tor_ci");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let genre = format!("s020-pending-projection-{nonce}");
    let search = PgSearch::new(pool.clone());
    let filters = CatalogFacets {
        term: "",
        language: None,
        genre: None,
        tag: None,
        fandom: None,
    };
    let before = search.count_catalog(filters).await.unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,
                           visibility_status,access_level,lifecycle_status,genres)
         VALUES($1,'ru',$2,'public','active','basic','pending_deletion',ARRAY[$3::text])
         RETURNING id",
    ).bind(format!("ci:s020:pending:projection:{nonce}"))
      .bind(format!("Pending projection {nonce}"))
      .bind(&genre).fetch_one(&pool).await.unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), before);
    assert!(!search.taxonomy_options().await.unwrap().genres.contains(&genre));
    assert_public_catalog_projection_count(&pool).await;

    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(id).execute(&pool).await.unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), before + 1);
    assert!(search.taxonomy_options().await.unwrap().genres.contains(&genre));
    assert_public_catalog_projection_count(&pool).await;

    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(id).execute(&pool).await.unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), before);
    assert!(!search.taxonomy_options().await.unwrap().genres.contains(&genre));
    assert_public_catalog_projection_count(&pool).await;

    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(id).execute(&pool).await.unwrap();
    assert_eq!(search.count_catalog(filters).await.unwrap(), before);
}
