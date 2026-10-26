use sqlx::{PgPool, Row};
use tor_search::{PgSearch, SearchError};

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_public_books_only_and_stable_cursor() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never modify live data");
    let row = sqlx::query(
        "INSERT INTO books (source_identity,target_language,title,visibility)
         VALUES ('ci:f03','ru','TOR Search Next','public')
         ON CONFLICT(source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second: i64 = row.try_get("id").unwrap();
    let search = PgSearch::new(pool);
    let first = search.search("TOR", None, 1).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, 1);
    assert_eq!(first.next_cursor.as_deref(), Some("1"));
    let following = search.search("TOR", Some("1"), 50).await.unwrap();
    assert!(following.items.iter().any(|item| item.id == second as u64));
    assert!(!following
        .items
        .iter()
        .any(|item| item.id == 2 || item.id == 3));
    assert!(search
        .search("' OR 1=1 --", None, 50)
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_language_filter_and_param_binding() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never mutate production DB");
    sqlx::query(
        "INSERT INTO books (source_identity,target_language,title,visibility)
         VALUES ('ci:f03-en','en','TOR English Search','public')
         ON CONFLICT (source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title",
    )
    .execute(&pool)
    .await
    .unwrap();
    let search = PgSearch::new(pool);
    let english = search
        .search_filtered("TOR", Some("EN"), None, 100)
        .await
        .unwrap();
    assert!(!english.items.is_empty());
    assert!(english.items.iter().all(|item| item.language == "en"));
    let russian = search
        .search_filtered("TOR", Some("ru"), None, 100)
        .await
        .unwrap();
    assert!(russian.items.iter().any(|item| item.id == 1));
    assert!(russian.items.iter().all(|item| item.language == "ru"));
    assert_eq!(
        search.search_filtered("TOR", Some("../ru"), None, 20).await,
        Err(SearchError::Invalid)
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL with public/hidden/held fixtures"]
async fn f_03_facet_search_masks_hidden_author_and_genre() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never modify production");
    let rows = [
        (
            "ci:f03-meta:visible",
            "Фэнтези рассвет",
            "Жюль",
            "public",
            false,
        ),
        (
            "ci:f03-meta:hidden",
            "Фэнтези закрыто",
            "Жюль",
            "draft",
            false,
        ),
        ("ci:f03-meta:held", "Фэнтези блок", "Жюль", "public", true),
    ];
    for (identity, title, author, visibility, held) in rows {
        sqlx::query(
            "INSERT INTO books(source_identity,target_language,title,author_name,genres,visibility,admin_hold)
             VALUES ($1,'ru',$2,$3,ARRAY['фэнтези','приключения'],$4,$5)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold",
        )
        .bind(identity)
        .bind(title)
        .bind(author)
        .bind(visibility)
        .bind(held)
        .execute(&pool)
        .await
        .unwrap();
    }
    let search = PgSearch::new(pool);
    let by_author = search
        .search_faceted("Жюль", Some("ru"), None, None, 30)
        .await
        .unwrap();
    assert!(by_author
        .items
        .iter()
        .any(|item| item.title == "Фэнтези рассвет"));
    assert!(!by_author
        .items
        .iter()
        .any(|item| item.title.contains("закрыто") || item.title.contains("блок")));
    let by_genre = search
        .search_faceted("", None, Some(" ФЭНТЕЗИ "), None, 30)
        .await
        .unwrap();
    assert!(by_genre
        .items
        .iter()
        .any(|item| item.title == "Фэнтези рассвет"));
    assert!(!by_genre
        .items
        .iter()
        .any(|item| item.title.contains("закрыто") || item.title.contains("блок")));
    assert_eq!(
        search
            .search_faceted("", None, Some("неизвестный жанр"), None, 30)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        search
            .search_faceted("Жюль", None, Some("неизвестный жанр"), None, 30)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_revocation_removes_author_genre_and_title_from_public_search() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci", "never alter production books");

    // Dedicated identity: this test checks the published -> draft/hold/deleted
    // transitions on the *same* row, without deleting shared fixtures.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books
         (source_identity,target_language,title,author_name,genres,visibility,admin_hold,is_deleted)
         VALUES ('ci:f03:revocation','ru','Отзывная книга','Уникальныйавтор',
                 ARRAY['уникальныйжанр'],'draft',FALSE,FALSE)
         ON CONFLICT (source_identity,target_language)
         DO UPDATE SET visibility='draft',admin_hold=FALSE,is_deleted=FALSE,
                       title=EXCLUDED.title,author_name=EXCLUDED.author_name,genres=EXCLUDED.genres
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let search = PgSearch::new(pool.clone());

    // Check three independent discovery dimensions. None may reveal a draft.
    for (term, genre) in [
        ("Отзывная", None),
        ("Уникальныйавтор", None),
        ("", Some("уникальныйжанр")),
    ] {
        let found = search
            .search_faceted(term, Some("ru"), genre, None, 100)
            .await
            .unwrap();
        assert!(!found.items.iter().any(|book| book.id == id as u64));
    }

    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let published = search
        .search_faceted("", Some("ru"), Some("уникальныйжанр"), None, 100)
        .await
        .unwrap();
    assert!(published.items.iter().any(|book| book.id == id as u64));

    // A hold must revoke existing search results immediately, with no stale
    // index-only result leaking author metadata or even the book identity.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let held = search
        .search_faceted("Уникальныйавтор", None, None, None, 100)
        .await
        .unwrap();
    assert!(!held.items.iter().any(|book| book.id == id as u64));

    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let withdrawn = search
        .search_faceted("Отзывная", None, None, None, 100)
        .await
        .unwrap();
    assert!(!withdrawn.items.iter().any(|book| book.id == id as u64));

    sqlx::query("UPDATE books SET visibility='public',is_deleted=TRUE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let deleted = search
        .search_faceted("", None, Some("уникальныйжанр"), None, 100)
        .await
        .unwrap();
    assert!(!deleted.items.iter().any(|book| book.id == id as u64));
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_04_live_genre_showcase_revokes_private_books_before_limit() {
    use tor_search::{repositories::showcases::PgShowcases, services::showcases::RecentShowcases};
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci", "must never mutate a production database");

    // Insert the public book before two larger private IDs. Filtering must
    // happen in SQL before LIMIT 1, not in the view/service after paging.
    let rows = [
        ("ci:f04:visible", "F04 доступно", "public", false),
        ("ci:f04:held", "F04 блокировано", "public", true),
        ("ci:f04:draft", "F04 черновик", "draft", false),
    ];
    let mut visible_id = 0_i64;
    for (source, title, visibility, held) in rows {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books
                (source_identity,target_language,title,author_name,genres,visibility,admin_hold,is_deleted)
             VALUES ($1,'ru',$2,'Автор F04',ARRAY['f04-test-genre'],$3,$4,FALSE)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,genres=EXCLUDED.genres,
                           author_name=EXCLUDED.author_name,
                           visibility=EXCLUDED.visibility,admin_hold=EXCLUDED.admin_hold,
                           is_deleted=FALSE RETURNING id",
        )
        .bind(source)
        .bind(title)
        .bind(visibility)
        .bind(held)
        .fetch_one(&pool)
        .await
        .unwrap();
        if source == "ci:f04:visible" {
            visible_id = id;
        }
    }

    let repository = PgShowcases::new(pool.clone());
    let service = RecentShowcases::new(&repository);
    let listed = service
        .latest_added(Some(" F04-TEST-GENRE "), 1)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, visible_id as u64);
    assert_eq!(listed[0].author_name, "Автор F04");
    assert_eq!(listed[0].genres, vec!["f04-test-genre".to_owned()]);

    // No cache: each transition must be reflected in the very next request.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(visible_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(service
        .latest_added(Some("f04-test-genre"), 1)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=$1")
        .bind(visible_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(service
        .latest_added(Some("f04-test-genre"), 1)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE books SET visibility='public',is_deleted=TRUE WHERE id=$1")
        .bind(visible_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(service
        .latest_added(Some("f04-test-genre"), 1)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_02_both_keyset_orders_skip_private_rows_without_gaps_or_duplicates() {
    use std::collections::HashSet;

    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never mutate production");

    // 27 published + 3 larger private IDs, all carrying the same genre.
    // Their source identities isolate this case from concurrently running tests.
    let mut expected = Vec::<u64>::new();
    for index in 0..30 {
        let identity = format!("ci:f02:keyset:{index:03}");
        let visible = index < 27;
        let visibility = if index == 27 { "draft" } else { "public" };
        let held = index == 28;
        let deleted = index == 29;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books (source_identity,target_language,title,author_name,genres,visibility,admin_hold,is_deleted)
             VALUES ($1,'ru',$2,'Keyset Author',ARRAY['f02-keyset'],$3,$4,$5)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold,is_deleted=EXCLUDED.is_deleted
             RETURNING id",
        )
        .bind(&identity)
        .bind(format!("F02 Book {index:03}"))
        .bind(visibility)
        .bind(held)
        .bind(deleted)
        .fetch_one(&pool)
        .await
        .unwrap();
        if visible {
            expected.push(u64::try_from(id).unwrap());
        }
    }
    expected.sort_unstable();
    let repo = PgSearch::new(pool);
    for (sort, ascending) in [("oldest", true), ("newest", false)] {
        let mut actual = Vec::new();
        let mut cursor: Option<String> = None;
        let mut visited = HashSet::new();
        for _ in 0..10 {
            let page = repo
                .search_faceted_sorted(
                    "",
                    Some("ru"),
                    Some("f02-keyset"),
                    cursor.as_deref(),
                    5,
                    Some(sort),
                )
                .await
                .unwrap();
            assert!(page.items.len() <= 5);
            for item in page.items {
                assert!(visited.insert(item.id), "duplicate on {sort}");
                actual.push(item.id);
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let mut expected_for_sort = expected.clone();
        if !ascending {
            expected_for_sort.reverse();
        }
        assert_eq!(actual, expected_for_sort, "cursor order {sort}");
    }
    assert_eq!(
        repo.search_faceted_sorted("", None, None, None, 20, Some("bogus"))
            .await,
        Err(SearchError::Invalid),
    );
    assert_eq!(
        repo.search_faceted_sorted("", None, None, Some("01"), 20, Some("newest"))
            .await,
        Err(SearchError::Invalid),
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_02_exact_total_matches_filters_and_immediate_revocation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never modify production");

    let mut public_ru_id = 0_i64;
    for (source, language, title, visibility, held, deleted) in [
        ("ru-a", "ru", "F02CountAlpha", "public", false, false),
        ("ru-b", "ru", "F02CountBeta", "public", false, false),
        ("en-a", "en", "F02CountAlpha", "public", false, false),
        ("ru-d", "ru", "F02CountAlpha", "draft", false, false),
        ("ru-h", "ru", "F02CountAlpha", "public", true, false),
        ("ru-x", "ru", "F02CountAlpha", "public", false, true),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books (source_identity,target_language,title,author_name,genres,
                                visibility,admin_hold,is_deleted)
             VALUES ($1,$2,$3,'F02 Count Author',ARRAY['f02-count-total'],$4,$5,$6)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold,is_deleted=EXCLUDED.is_deleted
             RETURNING id",
        )
        .bind(format!("ci:f02:count:{source}"))
        .bind(language)
        .bind(title)
        .bind(visibility)
        .bind(held)
        .bind(deleted)
        .fetch_one(&pool)
        .await
        .unwrap();
        if source == "ru-a" {
            public_ru_id = id;
        }
    }
    let search = PgSearch::new(pool.clone());
    assert_eq!(
        search
            .count_faceted("", None, Some("f02-count-total"))
            .await,
        Ok(3)
    );
    assert_eq!(
        search
            .count_faceted("", Some(" RU "), Some(" F02-COUNT-TOTAL "))
            .await,
        Ok(2)
    );
    assert_eq!(
        search
            .count_faceted("F02CountAlpha", None, Some("f02-count-total"))
            .await,
        Ok(2)
    );
    assert_eq!(
        search
            .count_faceted("F02CountAlpha", Some("ru"), Some("f02-count-total"))
            .await,
        Ok(1)
    );
    assert_eq!(
        search
            .count_faceted("", Some("de"), Some("f02-count-total"))
            .await,
        Ok(0)
    );
    assert_eq!(
        search
            .count_faceted("", None, Some("f02-count-total%"))
            .await,
        Ok(0)
    );
    assert_eq!(
        search.count_faceted("", None, Some("bad\ngenre")).await,
        Err(SearchError::Invalid)
    );

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(public_ru_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        search
            .count_faceted("", Some("ru"), Some("f02-count-total"))
            .await,
        Ok(1),
        "hold must immediately change total"
    );
    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=$1")
        .bind(public_ru_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        search
            .count_faceted("", Some("ru"), Some("f02-count-total"))
            .await,
        Ok(1),
        "draft must not reappear in total"
    );
    sqlx::query("UPDATE books SET visibility='public',is_deleted=TRUE WHERE id=$1")
        .bind(public_ru_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        search
            .count_faceted("", Some("ru"), Some("f02-count-total"))
            .await,
        Ok(1),
        "deleted must not reappear in total"
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_02_lexical_keysets_match_postgres_order_and_hide_private_rows() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never write to production");
    for (key, title, author, visibility, held, deleted) in [
        ("a", "Море", "Иван", "public", false, false),
        ("b", "Яблоко", "Анна", "public", false, false),
        ("c", "море", "Анна", "public", false, false),
        ("d", "яблоко", "Борис", "public", false, false),
        ("e", "Apple", "", "public", false, false),
        ("f", "Apple", "", "public", false, false),
        ("draft", "A-hidden", "A-hidden", "draft", false, false),
        ("held", "A-held", "A-held", "public", true, false),
        ("deleted", "A-deleted", "A-deleted", "public", false, true),
    ] {
        sqlx::query(
            "INSERT INTO books (source_identity,target_language,title,author_name,genres,
                                visibility,admin_hold,is_deleted)
             VALUES ($1,'ru',$2,$3,ARRAY['f02-lexical'],$4,$5,$6)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold,is_deleted=EXCLUDED.is_deleted",
        )
        .bind(format!("ci:f02:lexical:{key}"))
        .bind(title)
        .bind(author)
        .bind(visibility)
        .bind(held)
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    }
    let search = PgSearch::new(pool.clone());
    for (sort, oracle) in [
        (
            "title",
            "SELECT id FROM books WHERE visibility='public'
               AND NOT admin_hold AND NOT is_deleted
               AND genres @> ARRAY['f02-lexical']
               ORDER BY lower(title) ASC,id ASC",
        ),
        (
            "author",
            "SELECT id FROM books WHERE visibility='public'
               AND NOT admin_hold AND NOT is_deleted
               AND genres @> ARRAY['f02-lexical']
               ORDER BY lower(author_name) ASC,id ASC",
        ),
    ] {
        let expected: Vec<i64> = sqlx::query_scalar(oracle).fetch_all(&pool).await.unwrap();
        assert_eq!(expected.len(), 6);
        let mut actual = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = search
                .search_faceted_sorted(
                    "",
                    Some("ru"),
                    Some("f02-lexical"),
                    cursor.as_deref(),
                    2,
                    Some(sort),
                )
                .await
                .unwrap();
            assert!(!page.items.is_empty());
            assert!(page.items.len() <= 2);
            actual.extend(page.items.into_iter().map(|book| i64::try_from(book.id).unwrap()));
            pages += 1;
            assert!(pages <= 5, "cursor loop for {sort}");
            match page.next_cursor {
                Some(next) => {
                    assert!(next.starts_with(&format!("{sort}.")));
                    assert_ne!(cursor.as_deref(), Some(next.as_str()));
                    cursor = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(pages, 3);
        assert_eq!(actual, expected, "{sort}: unstable order or private row");
    }
    for (sort, cursor) in [
        ("title", "1"),
        ("author", "title.6162.1"),
        ("title", "title.gg.1"),
        ("title", "title.0.1"),
        ("title", "title.00.0"),
        ("author", "author.00.01"),
    ] {
        assert_eq!(
            search
                .search_faceted_sorted(
                    "",
                    Some("ru"),
                    Some("f02-lexical"),
                    Some(cursor),
                    2,
                    Some(sort),
                )
                .await,
            Err(SearchError::Invalid),
            "bad {sort} cursor {cursor}"
        );
    }
    let excessive = format!("title.{}.1", "a".repeat(2049));
    assert_eq!(
        search
            .search_faceted_sorted(
                "",
                Some("ru"),
                Some("f02-lexical"),
                Some(&excessive),
                2,
                Some("title"),
            )
            .await,
        Err(SearchError::Invalid),
    );
}
