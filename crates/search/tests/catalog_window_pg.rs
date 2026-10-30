//! B-18: forward and reverse keysets match PostgreSQL for all four sort modes.
//! Only a disposable tor_ci database; never a user or production database.
use sqlx::PgPool;
use tor_search::{
    repositories::{CatalogFacets, PgSearch},
    SearchError,
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn b_18_47_public_books_forward_backward_no_skip_or_duplicate() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG");
    assert!(url.ends_with("/tor_ci"), "never access a retained database");
    let pool = PgPool::connect(&url).await.expect("connect");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci");
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let term = format!("PagerBook{nonce}");
    let source = format!("ci:b18:{nonce}");
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,author_name,
                genres,tags,fandoms,visibility,admin_hold,is_deleted)
         SELECT $1 || ':' || n::text,'ru',$2 || ' ' || lpad(n::text,3,'0'),
                $3 || (n%3)::text,
                ARRAY['pager-genre'],ARRAY['pager-tag'],ARRAY['pager-fandom'],
                CASE WHEN n<=47 THEN 'public' ELSE 'draft' END,n=49,n=50
         FROM generate_series(1,50) AS n",
    )
    .bind(&source)
    .bind(&term)
    .bind(format!("Author{nonce}"))
    .execute(&pool)
    .await
    .expect("seed 47 visible and 3 hidden");

    let search = PgSearch::new(pool.clone());
    for (sort, order_by) in [
        ("oldest", "id ASC"),
        ("newest", "id DESC"),
        ("title", "lower(title) ASC,id ASC"),
        ("author", "lower(author_name) ASC,id ASC"),
    ] {
        let oracle_sql = format!(
            "SELECT id FROM books WHERE source_identity LIKE $1
             AND visibility='public' AND NOT admin_hold AND NOT is_deleted
             ORDER BY {order_by}"
        );
        let expected: Vec<i64> = sqlx::query_scalar(&oracle_sql)
            .bind(format!("{source}:%"))
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(expected.len(), 47, "{sort}");

        let facets = CatalogFacets {
            term: &term,
            language: Some("ru"),
            genre: Some("pager-genre"),
            tag: Some("pager-tag"),
            fandom: Some("pager-fandom"),
        };
        let first = search
            .browse_catalog_page(facets, None, None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(first.items.len(), 20, "{sort} first");
        assert!(first.previous_cursor.is_none());
        assert!(first.next_cursor.is_some());
        let second = search
            .browse_catalog_page(facets, first.next_cursor.as_deref(), None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(second.items.len(), 20, "{sort} second");
        assert!(second.previous_cursor.is_some());
        assert!(second.next_cursor.is_some());
        let third = search
            .browse_catalog_page(facets, second.next_cursor.as_deref(), None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(third.items.len(), 7, "{sort} third");
        assert!(third.previous_cursor.is_some());
        assert!(third.next_cursor.is_none());
        let combined = [&first.items[..], &second.items[..], &third.items[..]]
            .concat()
            .into_iter()
            .map(|book| i64::try_from(book.id).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(combined, expected, "{sort}: exactly 47 without gaps");

        let back2 = search
            .browse_catalog_page(
                facets,
                None,
                third.previous_cursor.as_deref(),
                20,
                Some(sort),
            )
            .await
            .unwrap();
        assert_eq!(back2.items, second.items, "{sort}: page 3 -> 2");
        assert!(back2.next_cursor.is_some());
        assert!(back2.previous_cursor.is_some());
        let forward3 = search
            .browse_catalog_page(facets, back2.next_cursor.as_deref(), None, 20, Some(sort))
            .await
            .unwrap();
        assert_eq!(forward3.items, third.items, "{sort}: page 2 -> 3");
        let back1 = search
            .browse_catalog_page(
                facets,
                None,
                back2.previous_cursor.as_deref(),
                20,
                Some(sort),
            )
            .await
            .unwrap();
        assert_eq!(back1.items, first.items, "{sort}: page 2 -> 1");
        assert!(back1.previous_cursor.is_none());
        assert!(back1.next_cursor.is_some());

        assert!(matches!(
            search
                .browse_catalog_page(facets, Some("1"), Some("2"), 20, Some(sort))
                .await,
            Err(SearchError::Invalid)
        ));
        assert!(matches!(
            search
                .browse_catalog_page(facets, None, Some("bad-cursor"), 20, Some(sort))
                .await,
            Err(SearchError::Invalid)
        ));
    }
    pool.close().await;
}
