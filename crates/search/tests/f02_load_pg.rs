//! Isolated F-02 load proof. Run explicitly on the disposable tor_ci database.
//! Does not run as part of the ordinary PostgreSQL test suite.
use std::time::Instant;

use sqlx::PgPool;
use tor_search::repositories::PgSearch;

#[tokio::test]
#[ignore = "explicit F-02 load job only: inserts 100,000 disposable books"]
async fn f_02_100k_keyset_order_visibility_and_query_plans() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    let pool = PgPool::connect(&url).await.expect("connect PostgreSQL");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "load test must never touch production");

    let seed_start = Instant::now();
    let inserted = sqlx::query(
        "INSERT INTO books
         (source_identity, target_language, title, author_name, genres,
          visibility, admin_hold, is_deleted)
         SELECT 'ci:f02:100k:' || n::text, 'ru', 'F02 Load Book ' || n::text,
                'F02 Load Author', ARRAY['f02-load'],
                CASE WHEN n % 20 = 0 THEN 'draft' ELSE 'public' END,
                n % 20 = 1, n % 20 = 2
         FROM generate_series(1, 100000) AS n",
    )
    .execute(&pool)
    .await
    .expect("seed 100k books")
    .rows_affected();
    assert_eq!(inserted, 100_000);
    println!("F02_100K seed_ms={}", seed_start.elapsed().as_millis());

    // The app role does not own the table and must not receive DDL privileges.
    // Use the isolated schema-owner connection only for planner statistics.
    let migrator_url = std::env::var("MIGRATOR_URL").expect("test migrator URL");
    let migrator = PgPool::connect(&migrator_url).await.unwrap();
    let identity: (String, String) = sqlx::query_as("SELECT current_database(), current_user")
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert_eq!(identity, ("tor_ci".to_owned(), "tor_migrator".to_owned()));
    sqlx::query("ANALYZE books")
        .execute(&migrator)
        .await
        .expect("refresh planner statistics");

    // Independent SQL oracle, not the search repository under test.
    let expected: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM books WHERE visibility='public'
          AND NOT admin_hold AND NOT is_deleted
          AND genres @> ARRAY['f02-load'] AND target_language='ru'
          ORDER BY id ASC",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(expected.len(), 85_000);

    let search = PgSearch::new(pool.clone());
    for (sort, oldest_first) in [("oldest", true), ("newest", false)] {
        let started = Instant::now();
        let mut seen = Vec::with_capacity(expected.len());
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let response = search
                .search_faceted_sorted(
                    "",
                    Some("ru"),
                    Some("f02-load"),
                    cursor.as_deref(),
                    100,
                    Some(sort),
                )
                .await
                .unwrap();
            assert!(!response.items.is_empty(), "empty intermediate {sort} page");
            assert!(response.items.len() <= 100, "page limit exceeded");
            let last = response.items.last().unwrap().id.to_string();
            for book in response.items {
                assert_eq!(book.language, "ru");
                assert!(book.genres.iter().any(|genre| genre == "f02-load"));
                seen.push(i64::try_from(book.id).unwrap());
            }
            pages += 1;
            assert!(pages <= 900, "{sort}: cursor loop or unbounded paging");
            match response.next_cursor {
                Some(next) => {
                    assert_eq!(next, last, "{sort}: cursor must point at last row");
                    assert_ne!(cursor.as_deref(), Some(next.as_str()));
                    cursor = Some(next);
                }
                None => break,
            }
        }
        let mut ordered = expected.clone();
        if !oldest_first {
            ordered.reverse();
        }
        assert_eq!(
            seen, ordered,
            "{sort}: duplicate, missing, hidden or disordered row"
        );
        assert_eq!(pages, 850);
        println!(
            "F02_100K sort={sort} pages={pages} visible={} scan_ms={}",
            seen.len(),
            started.elapsed().as_millis()
        );
    }

    // Measure the exact count separately; it is NOT yet exposed by the HTTP API.
    let count_start = Instant::now();
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM books WHERE visibility='public'
         AND NOT admin_hold AND NOT is_deleted
         AND genres @> ARRAY['f02-load'] AND target_language='ru'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, 85_000);
    println!(
        "F02_100K exact_total={total} count_ms={}",
        count_start.elapsed().as_millis()
    );
    // Test the real opt-in total path, not only a standalone SQL count.
    let counted_start = Instant::now();
    assert_eq!(
        search
            .count_faceted("", Some("ru"), Some("f02-load"))
            .await
            .unwrap(),
        85_000
    );
    assert_eq!(search.count_faceted("", None, None).await.unwrap(), 85_000);
    assert_eq!(
        search
            .count_faceted("F02", Some("ru"), Some("f02-load"))
            .await
            .unwrap(),
        85_000
    );
    assert_eq!(
        search
            .count_faceted("", Some("en"), Some("f02-load"))
            .await
            .unwrap(),
        0
    );
    println!(
        "F02_100K filtered_count_checks_ms={}",
        counted_start.elapsed().as_millis()
    );

    // NEwBOO-style lexical sorts must also traverse 100k rows without OFFSET,
    // including duplicate author names, not only pass on a small fixture.
    for (sort, oracle) in [
        (
            "title",
            "SELECT id FROM books WHERE visibility='public'
              AND NOT admin_hold AND NOT is_deleted
              AND target_language='ru' AND genres @> ARRAY['f02-load']
              ORDER BY lower(title) ASC,id ASC",
        ),
        (
            "author",
            "SELECT id FROM books WHERE visibility='public'
              AND NOT admin_hold AND NOT is_deleted
              AND target_language='ru' AND genres @> ARRAY['f02-load']
              ORDER BY lower(author_name) ASC,id ASC",
        ),
    ] {
        let expected: Vec<i64> = sqlx::query_scalar(oracle).fetch_all(&pool).await.unwrap();
        assert_eq!(expected.len(), 85_000);
        let started = Instant::now();
        let mut actual = Vec::with_capacity(expected.len());
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = search
                .search_faceted_sorted(
                    "",
                    Some("ru"),
                    Some("f02-load"),
                    cursor.as_deref(),
                    100,
                    Some(sort),
                )
                .await
                .unwrap();
            assert!(!page.items.is_empty());
            assert!(page.items.len() <= 100);
            actual.extend(
                page.items
                    .into_iter()
                    .map(|book| i64::try_from(book.id).unwrap()),
            );
            pages += 1;
            assert!(pages <= 900, "unbounded {sort} pagination");
            match page.next_cursor {
                Some(next) => {
                    assert!(next.starts_with(&format!("{sort}.")));
                    assert_ne!(cursor.as_deref(), Some(next.as_str()));
                    cursor = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(pages, 850);
        assert_eq!(
            actual, expected,
            "{sort}: missing, duplicate or private row"
        );
        println!(
            "F02_100K lexical_sort={sort} pages={pages} visible={} scan_ms={}",
            actual.len(),
            started.elapsed().as_millis()
        );
    }

    // Keep 100k mixed-visibility books; project taxonomy through the exact
    // same bounded keyset and count paths, without OFFSET.
    sqlx::query(
        "UPDATE books SET tags=ARRAY['f02-100k-tag'], fandoms=ARRAY['f02-100k-fandom']
         WHERE source_identity LIKE 'ci:f02:100k:%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let taxonomy = tor_search::repositories::CatalogFacets {
        term: "",
        language: Some("ru"),
        genre: Some("f02-load"),
        tag: Some("f02-100k-tag"),
        fandom: Some("f02-100k-fandom"),
    };
    assert_eq!(search.count_catalog(taxonomy).await.unwrap(), 85_000);
    let first = search
        .search_catalog(taxonomy, None, 100, Some("title"))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 100);
    assert!(first
        .items
        .iter()
        .all(|item| item.tags.iter().any(|value| value == "f02-100k-tag")
            && item.fandoms.iter().any(|value| value == "f02-100k-fandom")));
    println!(
        "F02_100K taxonomy_total=85000 first_page={}",
        first.items.len()
    );

    // F-03: fullwidth Unicode search reaches the same canonical token index.
    let unicode = search
        .search_faceted_sorted(
            "９９９９９", Some("ru"), Some("f02-load"), None, 5, Some("newest")
        ).await.unwrap();
    assert_eq!(unicode.items.len(), 1, "NFKC search must match book 99999");
    assert!(unicode.items[0].title.ends_with("99999"));
    assert_eq!(
        search.count_faceted("９９９９９", Some("ru"), Some("f02-load")).await,
        Ok(1)
    );
    println!("F03_100K nfkc_fullwidth_search=PASS");

    // These are the repository's two static query shapes, with representative
    // midpoint cursors. Record real PostgreSQL execution plans as CI evidence.
    let midpoint = expected[expected.len() / 2];
    for (sort, query) in [
        ("oldest", "EXPLAIN (ANALYZE, BUFFERS) SELECT id,title,target_language,author_name,genres
          FROM books WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
          AND ($1::text = '' OR
               (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
                   @@ plainto_tsquery('simple'::regconfig, $1))
          AND id > $2
          AND ($4::text IS NULL OR target_language = $4)
          AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
          ORDER BY id ASC LIMIT $3"),
        ("newest", "EXPLAIN (ANALYZE, BUFFERS) SELECT id,title,target_language,author_name,genres
          FROM books WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
          AND ($1::text = '' OR
               (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
                   @@ plainto_tsquery('simple'::regconfig, $1))
          AND ($2::bigint = 0 OR id < $2)
          AND ($4::text IS NULL OR target_language = $4)
          AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
          ORDER BY id DESC LIMIT $3"),
    ] {
        let lines: Vec<String> = sqlx::query_scalar(query)
            .bind("")
            .bind(midpoint)
            .bind(101_i64)
            .bind(Some("ru"))
            .bind(Some("f02-load"))
            .fetch_all(&pool)
            .await
            .expect("EXPLAIN actual catalog query");
        println!("F02_100K plan={sort}\n{}", lines.join("\n"));
        assert!(lines.iter().any(|line| line.contains("Limit")), "missing SQL limit");
        assert!(!lines.iter().any(|line| line.contains("Offset")), "OFFSET regression");
    }
}
