use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tor_books::repositories::PgCatalog;
use tower::ServiceExt;
async fn get(app: axum::Router, path: &str) -> (StatusCode, String) {
    let res = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 131072).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci"]
async fn pg_live_http_and_private_books() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    assert_eq!(get(app.clone(), "/readyz").await.0, StatusCode::OK);
    let (status, json) = get(app.clone(), "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("TOR CI public"));
    assert!(!json.contains("TOR CI hidden"));
    assert!(!json.contains("TOR CI held"));
    let (status, html) = get(app.clone(), "/catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("TOR CI public"));
    assert!(!html.contains("TOR CI hidden"));
    let (home_status, home_html) = get(app.clone(), "/").await;
    assert_eq!(home_status, StatusCode::OK);
    assert!(home_html.contains("Недавно добавленные"));
    assert!(home_html.contains("home-spotlight-card"));
    assert!(home_html.contains("home-shelf__track"));
    assert!(!home_html.contains("TOR CI hidden"));
    assert!(!home_html.contains("TOR CI held"));

    assert_eq!(get(app.clone(), "/books/1").await.0, StatusCode::OK);
    assert_eq!(get(app.clone(), "/books/2").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(app.clone(), "/books/3").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(app, "/api/v1/books?limit=101").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci test records"]
async fn public_reading_http_renders_safe_markdown_and_blocks_paid_text() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let catalog = PgCatalog::connect(&url).await.expect("tor_app");
    let reader = tor_reading::repositories::PgReader::connect(&url)
        .await
        .expect("tor_app");
    let app = tor_web::router_with_postgres_reading(catalog, reader);
    let (status, json) = get(app.clone(), "/api/v1/books/1/toc?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("\"chapter_no\":1"));
    assert!(json.contains("\"next_cursor\":\"1.101\""));
    assert!(!json.contains("PRIVATE-PAID-MARKER"));
    let (status, json) = get(app.clone(), "/api/v1/books/1/toc?limit=1&after=1.101").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("\"chapter_no\":3"), "{json}");
    assert!(json.contains("\"access_level\":\"paid\""));
    assert!(!json.contains("PRIVATE-PAID-MARKER"));
    for invalid in [
        "/api/v1/books/1/toc?limit=0",
        "/api/v1/books/1/toc?limit=101",
        "/api/v1/books/1/toc?after=1.0",
        "/books/1?after=bad",
    ] {
        assert_eq!(
            get(app.clone(), invalid).await.0,
            StatusCode::BAD_REQUEST,
            "{invalid}"
        );
    }
    assert_eq!(
        get(app.clone(), "/api/v1/books/2/toc").await.0,
        StatusCode::NOT_FOUND
    );
    let (status, page_two) = get(app.clone(), "/books/1?limit=1&after=1.101").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page_two.contains("Платная глава"));
    assert!(!page_two.contains("PRIVATE-PAID-MARKER"));
    let (status, book) = get(app.clone(), "/books/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(book.contains("Глава 1. Начало истории"));
    assert!(book.contains("href=\"/reader/101\""));
    assert!(book.contains("href=\"/reader/103\""));
    assert!(book.contains("Платная · нужен доступ"));
    assert!(!book.contains("PRIVATE-PAID-MARKER"));
    assert!(!book.contains("Секретный черновик"));
    assert!(!book.contains("Запланированная глава"));
    let (status, html) = get(app.clone(), "/reader/101").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<strong>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("href=\"/reader/103\""));
    assert!(html.contains("Следующая глава — платная"));
    assert!(!html.contains("href=\"/reader/108\""));
    assert!(!html.contains("PRIVATE-PAID-MARKER"));
    let (status, locked) = get(app.clone(), "/reader/103").await;
    assert_eq!(status, StatusCode::OK);
    assert!(locked.contains("Платная глава"));
    assert!(locked.contains("href=\"/login\""));
    assert!(locked.contains("href=\"/books/1#chapters\""));
    assert!(!locked.contains("PRIVATE-PAID-MARKER"));
    for denied in [102, 104, 105, 106, 107] {
        assert_eq!(
            get(app.clone(), &format!("/reader/{denied}")).await.0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn c_10_denied_reader_api_has_no_text_or_shared_cache() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let reader = tor_reading::repositories::PgReader::connect(&url)
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading(catalog, reader);
    for path in [
        "/reader/101",
        "/reader/102",
        "/reader/103",
        "/reader/104",
        "/books/2",
        "/api/v1/books/2/toc",
        "/api/v1/chapters/101/read",
        "/api/v1/chapters/103/read?purchase_id=999",
        "/api/v1/chapters/102/read?preview=1",
    ] {
        let reply = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let code = reply.status();
        assert_eq!(
            code,
            if path == "/reader/101" || path == "/reader/103" {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            },
            "{path}"
        );
        assert_eq!(reply.headers()["cache-control"], "no-store", "{path}");
        assert_eq!(reply.headers()["x-content-type-options"], "nosniff");
        let policy = reply.headers()["content-security-policy"].to_str().unwrap();
        if path == "/reader/103" {
            assert_eq!(reply.headers()["x-robots-tag"], "noindex, nofollow");
        }
        assert!(policy.contains("script-src 'none'"));
        assert!(policy.contains("frame-ancestors 'none'"));
        let body = to_bytes(reply.into_body(), 256 * 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains("PRIVATE-PAID-MARKER"));
        assert!(!text.contains("PRIVATE-DRAFT-MARKER"));
        if path == "/reader/103" {
            assert!(text.contains("Платная глава"));
            assert!(text.contains("href=\"/login\""));
        }
        if code == StatusCode::NOT_FOUND && path == "/books/2" {
            // Browser-only book 404 uses the NEwBOO recovery page, never a private body.
            assert!(text.contains("Страница не найдена"));
            assert!(text.contains("noindex, nofollow"));
            assert!(text.contains("href=\"/catalog\""));
        } else if code == StatusCode::NOT_FOUND {
            // Reader and API errors must still have empty bodies.
            assert!(text.is_empty(), "protected read error must not expose data");
        } else if path == "/reader/101" {
            // Only the published free chapter renders its escaped body.
            assert!(text.contains("&lt;script&gt;"));
            assert!(!text.contains("<script>"));
        } else {
            // Paid metadata cannot borrow or expose the free or paid body.
            assert_eq!(path, "/reader/103");
            assert!(text.contains("Для чтения нужен доступ"));
            assert!(!text.contains("&lt;script&gt;"));
            assert!(!text.contains("<script>"));
        }
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci and public/hidden/held fixtures"]
async fn f_03_search_api_is_bounded_and_never_exposes_drafts() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test URL");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    let (status, json) = get(app.clone(), "/api/v1/search/books?q=TOR&limit=20").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("TOR CI public"));
    assert!(json.contains(r#""id":"1""#));
    assert!(!json.contains("TOR CI hidden"));
    assert!(!json.contains("TOR CI held"));
    let (status, blank_language) = get(app.clone(), "/catalog?q=TOR&language=").await;
    assert_eq!(status, StatusCode::OK);
    assert!(blank_language.contains("TOR CI public"));
    let (status, language_only) = get(app.clone(), "/api/v1/search/books?language=ru").await;
    assert_eq!(status, StatusCode::OK);
    assert!(language_only.contains("TOR CI public"));
    let (status, html) = get(app.clone(), "/catalog?q=TOR&language=ru").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"role="search""#));
    assert!(html.contains(r#"name="language""#));
    assert!(!html.contains("TOR CI hidden"));
    for path in [
        "/api/v1/search/books?q=TOR&limit=101",
        "/api/v1/search/books?q=TOR&language=bad!lang",
        "/api/v1/search/books?q=TOR&genre=bad%0Agenre",
        "/api/v1/search/books?q=TOR&after=01",
        "/api/v1/search/books",
        "/catalog?q=TOR&genre=bad%0Agenre",
        "/catalog?q=TOR&language=ru&after=01",
    ] {
        let (status, _) = get(app.clone(), path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
}

#[tokio::test]
#[ignore = "requires tor_ci public, draft, held fixtures"]
async fn f_03_http_author_and_genre_facets_do_not_leak() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    let (status, response) = get(
        app.clone(),
        "/api/v1/search/books?genre=%D1%84%D1%8D%D0%BD%D1%82%D0%B5%D0%B7%D0%B8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("TOR CI public"));
    assert!(!response.contains("TOR CI hidden"));
    assert!(!response.contains("TOR CI held"));
    let (status, author) = get(
        app.clone(),
        "/api/v1/search/books?q=%D0%96%D1%8E%D0%BB%D1%8C",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(author.contains("TOR CI public"));
    assert!(!author.contains("TOR CI hidden"));
    let (status, html) = get(
        app,
        "/catalog?genre=%D1%84%D1%8D%D0%BD%D1%82%D0%B5%D0%B7%D0%B8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("TOR CI public"));
    assert!(html.contains("Жюль"));
    assert!(!html.contains("TOR CI hidden"));
}

#[tokio::test]
#[ignore = "requires isolated tor_ci with standard public/hidden/held fixtures"]
async fn f_02_catalog_sort_keeps_legacy_json_contract_and_rejects_bad_cursors() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());

    let (status, html) = get(app.clone(), "/catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("TOR CI public"));
    assert!(html.contains(r#"name="sort""#));
    assert!(!html.contains("TOR CI hidden"));
    assert!(!html.contains("TOR CI held"));

    let (status, newest) = get(app.clone(), "/catalog?sort=newest").await;
    assert_eq!(status, StatusCode::OK);
    assert!(newest.contains(r#"value="newest" selected"#));
    assert!(!newest.contains("TOR CI hidden"));
    let (status, json) = get(app.clone(), "/api/v1/search/books?sort=newest&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains(r#""next_cursor":"#));

    // The original books endpoint is still a JSON array, not a new envelope.
    let (status, legacy) = get(app.clone(), "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(legacy.starts_with('['));

    for path in [
        "/catalog?sort=bad",
        "/catalog?sort=newest&after=01",
        "/catalog?sort=newest&after=-1",
        "/api/v1/search/books?sort=bad",
        "/api/v1/search/books?sort=newest&limit=101",
        "/api/v1/search/books?sort=newest&after=01",
    ] {
        assert_eq!(
            get(app.clone(), path).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated tor_ci with standard public/hidden/held fixtures"]
async fn f_02_exact_total_html_and_opt_in_json_obey_public_visibility() {
    // The web crate deliberately does not own SQLx: use the same immutable
    // public fixture via HTTP that the repository integration tests use.
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());

    let (status, html) = get(app.clone(), "/catalog?q=TOR%20CI%20public&sort=oldest").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Найдено книг: 1"), "{html}");
    assert!(html.contains("TOR CI public"));
    assert!(!html.contains("TOR CI hidden"));
    assert!(!html.contains("TOR CI held"));

    let (status, json) = get(
        app.clone(),
        "/api/v1/search/books?q=TOR%20CI%20public&sort=newest&include_total=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains(r#""total":1"#), "{json}");
    assert!(json.contains("TOR CI public"));

    let (status, zero) = get(
        app.clone(),
        "/api/v1/search/books?q=TOR%20CI%20public&language=en&sort=oldest&include_total=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(zero.contains(r#""total":0"#), "{zero}");
    assert!(zero.contains(r#""items":[]"#), "{zero}");

    // The public catalog has multiple records in this PG fixture; a cursor
    // page must not add the expensive opt-in total unless explicitly requested.
    let (status, first) = get(
        app.clone(),
        "/api/v1/search/books?sort=newest&limit=1&include_total=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(first.contains(r#""total":"#), "{first}");
    let cursor = first
        .split(r#""next_cursor":""#)
        .nth(1)
        .expect("fixture must contain multiple public books")
        .split('"')
        .next()
        .unwrap();
    let (status, second) = get(
        app.clone(),
        &format!("/api/v1/search/books?sort=newest&limit=1&after={cursor}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !second.contains(r#""total":"#),
        "cursor request must stay cheap"
    );
    assert_eq!(
        get(
            app.clone(),
            &format!("/api/v1/search/books?sort=newest&limit=1&after={cursor}&include_total=true"),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
    );
    let (status, legacy) = get(app, "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        legacy.starts_with('['),
        "original endpoint must be a JSON array"
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci public books"]
async fn f_02_lexical_sort_is_bounded_and_uses_sort_specific_cursors_in_http() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    for sort in ["title", "author"] {
        let (status, html) = get(app.clone(), &format!("/catalog?sort={sort}")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            html.contains(&format!("value=\"{sort}\" selected")),
            "{sort}: sort option not selected"
        );
        let (status, json) = get(
            app.clone(),
            &format!("/api/v1/search/books?sort={sort}&limit=1"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let cursor = json
            .split(r#""next_cursor":""#)
            .nth(1)
            .expect("multiple public books are seeded")
            .split('"')
            .next()
            .unwrap();
        assert!(cursor.starts_with(&format!("{sort}.")));
        let (status, page_two) = get(
            app.clone(),
            &format!("/api/v1/search/books?sort={sort}&limit=1&after={cursor}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!page_two.contains(r#""items":[]"#));
    }
    for path in [
        "/catalog?sort=title&after=1",
        "/catalog?sort=author&after=title.6162.1",
        "/api/v1/search/books?sort=title&after=title.gg.1",
        "/api/v1/search/books?sort=title&after=title.00.0",
        "/api/v1/search/books?sort=author&after=author.00.01",
    ] {
        assert_eq!(
            get(app.clone(), path).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated tor_ci database"]
async fn f_02_http_taxonomy_filters_validate_and_never_change_legacy_api() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    let (status, html) = get(app.clone(), "/catalog?tag=f02-absent&fandom=f02-absent").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("name=\"tag\""), "{html}");
    assert!(html.contains("name=\"fandom\""), "{html}");
    assert!(html.contains("Найдено книг: 0"), "{html}");
    assert!(html.contains("Книги не найдены"), "{html}");
    let (status, json) = get(
        app.clone(),
        "/api/v1/search/books?tag=f02-absent&fandom=f02-absent&sort=newest&include_total=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains(r#""total":0"#), "{json}");
    assert!(json.contains(r#""items":[]"#), "{json}");
    for path in [
        "/catalog?tag=bad%0Atag",
        "/catalog?fandom=bad%2Cfandom",
        "/api/v1/search/books?tag=bad%0Atag",
        "/api/v1/search/books?fandom=bad%2Cfandom",
    ] {
        assert_eq!(
            get(app.clone(), path).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let (status, legacy) = get(app, "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(legacy.starts_with('['));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci public/hidden/held fixture"]
async fn f_03_nfkc_search_normalization_works_through_public_http() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    // Fullwidth Unicode letters (ＴＯＲ) normalize to the fixture's ASCII title.
    let term = "%EF%BC%B4%EF%BC%AF%EF%BC%B2";
    for path in [
        format!("/api/v1/search/books?q={term}&sort=newest&include_total=true"),
        format!("/catalog?q={term}&sort=newest"),
    ] {
        let (code, response) = get(app.clone(), &path).await;
        assert_eq!(code, StatusCode::OK, "{path}");
        assert!(response.contains("TOR CI public"), "{path}: {response}");
        assert!(!response.contains("TOR CI hidden"), "{path}");
        assert!(!response.contains("TOR CI held"), "{path}");
    }
    let (code, legacy) = get(app, "/api/v1/books").await;
    assert_eq!(code, StatusCode::OK);
    assert!(legacy.starts_with('['));
}

#[tokio::test]
#[ignore = "requires disposable isolated PostgreSQL tor_ci; never run on production"]
async fn seo_11_anonymous_page_access_tracks_publish_hold_unhold_and_revoke() {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sql(url: &str, query: &str) -> String {
        assert!(url.ends_with("/tor_ci"), "production database forbidden");
        let out = Command::new("psql")
            .args([
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "--dbname",
                url,
                "--command",
                query,
            ])
            .output()
            .expect("psql for isolated fixture");
        assert!(
            out.status.success(),
            "fixture query failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG test URL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.expect("tor_app pool");
    let reader = tor_reading::repositories::PgReader::connect(&url)
        .await
        .expect("tor_app reader");
    let app = tor_web::router_with_postgres_reading(catalog, reader);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book: i64 = sql(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:seo:availability:{nonce}','ru','SEO доступность {nonce}','public') RETURNING id"
        ),
    ).parse().expect("book id");
    let chapter: i64 = sql(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Открытая глава','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .expect("chapter id");
    let version: i64 = sql(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'SEO-AVAILABILITY-PUBLIC-{nonce}') RETURNING id"
        ),
    )
    .parse()
    .expect("chapter version id");
    sql(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let detail = format!("/books/{book}");
    let read = format!("/reader/{chapter}");

    let (status, html) = get(app.clone(), &detail).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "published book must be publicly reachable"
    );
    assert!(html.contains("SEO доступность"));
    let (status, html) = get(app.clone(), &read).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "free published chapter must be reachable"
    );
    assert!(html.contains("SEO-AVAILABILITY-PUBLIC-"));

    // A moderation hold must change HTTP immediately, not wait for a
    // sitemap rebuild or any SEO projection to be refreshed.
    sql(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let (status, html) = get(app.clone(), &detail).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(html.contains("noindex, nofollow"));
    assert!(!html.contains("SEO-AVAILABILITY-PUBLIC-"));
    let (status, html) = get(app.clone(), &read).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("SEO-AVAILABILITY-PUBLIC-"));

    // Restoring a book makes its canonical public URLs reachable again.
    sql(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    assert_eq!(get(app.clone(), &detail).await.0, StatusCode::OK);
    assert_eq!(get(app.clone(), &read).await.0, StatusCode::OK);

    // Book revocation and chapter revocation are independent authorities.
    sql(
        &url,
        &format!("UPDATE chapters SET visibility='draft' WHERE id={chapter}"),
    );
    assert_eq!(get(app.clone(), &detail).await.0, StatusCode::OK);
    let (status, html) = get(app.clone(), &read).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("SEO-AVAILABILITY-PUBLIC-"));

    sql(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={book}"),
    );
    let (status, html) = get(app.clone(), &detail).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(html.contains("noindex, nofollow"));
    assert!(!html.contains("SEO-AVAILABILITY-PUBLIC-"));
    let (status, html) = get(app.clone(), &read).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("SEO-AVAILABILITY-PUBLIC-"));
    // User-specific purchased-version read paths are not changed by SEO.
}
