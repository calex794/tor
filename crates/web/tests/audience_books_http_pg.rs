//! FLOW-05: private book report, filter round-trips and bounded failure.
//! Daily rows are explicit report fixtures; actual collection is covered by Chromium.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    io::{BufRead, BufReader, Write},
    num::NonZeroU32,
    process::{Child, ChildStdin, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

fn client(url: &str) -> Command {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let mut command = Command::new("psql");
    command
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000",
        )
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url]);
    command
}

fn pg(url: &str, sql: &str) -> String {
    let output = client(url).args(["--command", sql]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Fixtures {
    url: String,
    books: Vec<i64>,
    users: Vec<i64>,
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        if !self.books.is_empty() {
            let ids = self
                .books
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            pg(
                &self.url,
                &format!(
                    "BEGIN; DELETE FROM rec_quality_daily WHERE book_id IN({ids});
                 DELETE FROM organic_discovery_quality_daily WHERE book_id IN({ids});
                 DELETE FROM rec_dirty_buckets WHERE book_id IN({ids});
                 DELETE FROM organic_discovery_dirty WHERE book_id IN({ids});
                 DELETE FROM books WHERE id IN({ids}); COMMIT"
                ),
            );
        }
        if !self.users.is_empty() {
            let ids = self
                .users
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            pg(
                &self.url,
                &format!(
                    "BEGIN; DELETE FROM account_roles WHERE user_id IN({ids});
                 DELETE FROM users WHERE id IN({ids}); COMMIT"
                ),
            );
        }
    }
}

struct LockedReport {
    child: Child,
    input: ChildStdin,
}

impl LockedReport {
    fn acquire(url: &str) -> Self {
        let mut child = client(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "BEGIN; LOCK TABLE organic_discovery_quality_daily IN ACCESS EXCLUSIVE MODE; SELECT 'books-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "books-locked");
        Self { child, input }
    }
}

impl Drop for LockedReport {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

async fn call(app: &Router, path: &str, cookie: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn private(response: &Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
}

fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn metric<'a>(html: &'a str, key: &str) -> &'a str {
    let marker = format!("data-metric=\"{key}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("rendered metric")
        .split('>')
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .trim()
}

fn metrics(html: &str, expected: [&str; 5]) {
    for (key, expected) in ["issued", "impressions", "clicks", "book-opens", "starts"]
        .iter()
        .zip(expected)
    {
        assert_eq!(metric(html, key), expected, "{key}");
    }
}

fn summary<'a>(html: &'a str, period: &str) -> &'a str {
    let marker = format!("data-testid=\"audience-books-{period}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("whole-filter summary")
}

fn row<'a>(html: &'a str, book: i64, period: &str) -> &'a str {
    let marker = format!("data-book-id=\"{book}\"");
    let section = html
        .split(marker.as_str())
        .nth(1)
        .expect("public book row")
        .split("data-testid=\"audience-book-row\"")
        .next()
        .unwrap();
    let marker = format!("data-period=\"{period}\"");
    section.split(marker.as_str()).nth(1).expect("book period")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn books_http_public_filters_paging_unknown_history_and_live_admin() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut fixtures = Fixtures {
        url: url.clone(),
        books: Vec::new(),
        users: Vec::new(),
    };
    let password = hash_password("AudienceBooksTestPassword2026").unwrap();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for kind in ["admin", "reader", "moderator"] {
        let token = generate_session_token();
        fixtures.users.push(
            accounts
                .register_with_session(
                    &format!("flow-books-{kind}-{stamp}@example.invalid"),
                    "Books report fixture",
                    &password,
                    &session_token_digest(&token),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={token}"));
        tokens.push(token);
    }
    let (admin, moderator) = (fixtures.users[0], fixtures.users[2]);
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"));
    // Unique filters isolate this report fixture from all preceding PG suites.
    let genre = format!("жанр & {stamp}");
    let tag = "сны + \"цитаты\"";
    for index in 0..24 {
        let title = if index == 0 {
            "<script>alert(1)</script> & книга".to_owned()
        } else {
            format!("FLOW-BOOK-{stamp}-{index}")
        };
        fixtures.books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
             VALUES('ci:flow:books:http:{stamp}:{index}','ru','{title}','public','basic',
             ARRAY['{genre}'],ARRAY['{tag}']) RETURNING id"
        )).parse().unwrap());
    }
    let [read, observed, direct, hidden] = [
        fixtures.books[0],
        fixtures.books[1],
        fixtures.books[2],
        fixtures.books[3],
    ];
    let secret_genre = format!("secret-genre-{stamp}");
    pg(&url, &format!(
        "UPDATE books SET admin_hold=true,genres=ARRAY['{genre}','{secret_genre}'],tags=ARRAY['{tag}','secret-tag-{stamp}'] WHERE id={hidden};
         INSERT INTO organic_discovery_quality_daily(bucket_date,book_id,source,issued,clicks,book_opens,starts,completions) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{read},'catalog',10,3,2,1,0),
         ((now() AT TIME ZONE 'UTC')::date,{read},'search',6,2,2,1,0),
         ((now() AT TIME ZONE 'UTC')::date,{observed},'catalog',3,0,0,0,0),
         ((now() AT TIME ZONE 'UTC')::date,{direct},'unattributed',0,0,0,1,0),
         ((now() AT TIME ZONE 'UTC')::date,{hidden},'catalog',999999,999999,999999,999999,999999),
         ((now() AT TIME ZONE 'UTC')::date-7,{read},'catalog',4,2,1,1,0);
         INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
         impressions,paired_clicks,unpaired_clicks,starts,completions,book_opens) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{read},'home','fallback','popular',1,'flow-http','v1',7,3,1,1,0,3),
         ((now() AT TIME ZONE 'UTC')::date-7,{read},'home','fallback','popular',1,'flow-http','v1',2,1,0,0,0,NULL)"
    ));
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        "http://127.0.0.1:3098".into(),
    );
    let path = format!(
        "/admin/audience/books?days=7&genre={}&tag={}",
        encoded(&genre),
        encoded(tag)
    );
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let denied = call(&app, "/admin/audience/books?days=invalid", cookie).await;
        private(&denied, status);
        assert!(!body(denied).await.contains("data-book-id="));
    }
    for raw in [
        "days=2",
        "days=x",
        "days=7&days=1",
        "after_id=0",
        "after_id=-1",
        "after_id=abc",
        "user_id=5",
        "genre=%00",
    ] {
        private(
            &call(&app, &format!("/admin/audience/books?{raw}"), &cookies[0]).await,
            StatusCode::BAD_REQUEST,
        );
    }
    let response = call(&app, &path, &cookies[0]).await;
    private(&response, StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; книга"));
    assert!(!html.contains("<script>alert(1)"));
    for forbidden in [
        format!("FLOW-BOOK-{stamp}-3"),
        secret_genre,
        format!("secret-tag-{stamp}"),
        format!("flow-books-admin-{stamp}"),
        tokens[0].clone(),
        "999999".into(),
    ] {
        assert!(
            !html.contains(&forbidden),
            "private or excluded value {forbidden}"
        );
    }
    assert_eq!(
        html.matches("data-testid=\"audience-book-row\"").count(),
        20
    );
    metrics(summary(&html, "current"), ["19", "7", "9", "7", "4"]);
    metrics(
        summary(&html, "previous"),
        ["4", "2", "3", "Нет данных", "1"],
    );
    metrics(row(&html, read, "current"), ["16", "7", "9", "7", "3"]);
    metrics(
        row(&html, read, "previous"),
        ["4", "2", "3", "Нет данных", "1"],
    );
    assert_eq!(metric(row(&html, observed, "current"), "starts"), "0");
    assert_eq!(metric(row(&html, direct, "current"), "starts"), "1");
    let next_tag = html
        .split('<')
        .find(|tag| tag.contains("data-testid=\"audience-books-next\""))
        .expect("next page link");
    let next = next_tag
        .split("href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .replace("&amp;", "&");
    assert!(
        next.contains("after_id=")
            && next.contains("days=7")
            && next.contains("genre=")
            && next.contains("tag=")
    );
    let second = call(&app, &next, &cookies[0]).await;
    private(&second, StatusCode::OK);
    let second = body(second).await;
    assert_eq!(
        second.matches("data-testid=\"audience-book-row\"").count(),
        3
    );
    assert!(!second.contains(&format!("data-book-id=\"{read}\"")));
    metrics(summary(&second, "current"), ["19", "7", "9", "7", "4"]);
    assert!(!second.contains("data-testid=\"audience-books-next\""));
    // Repeated report requests cannot write analytics or produce a new count.
    metrics(
        summary(&body(call(&app, &path, &cookies[0]).await).await, "current"),
        ["19", "7", "9", "7", "4"],
    );
    for days in [1, 30, 90] {
        private(
            &call(
                &app,
                &path.replace("days=7", &format!("days={days}")),
                &cookies[0],
            )
            .await,
            StatusCode::OK,
        );
    }
    // Hiding a current book removes metadata and its contribution immediately.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={read}"),
    );
    let updated = body(call(&app, &path, &cookies[0]).await).await;
    assert!(!updated.contains("alert(1)"));
    metrics(
        summary(&updated, "current"),
        ["3", "Нет данных", "0", "0", "1"],
    );

    let lock = LockedReport::acquire(&url);
    let unavailable = call(&app, &path, &cookies[0]).await;
    private(&unavailable, StatusCode::SERVICE_UNAVAILABLE);
    let unavailable = body(unavailable).await;
    assert!(unavailable.contains("audience-books-unavailable"));
    assert!(!unavailable.contains("data-book-id="));
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    private(&call(&app, &path, &cookies[0]).await, StatusCode::FORBIDDEN);
    drop(lock);
    pg(&url, &format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
    private(&call(&app, &path, &cookies[0]).await, StatusCode::OK);
    accounts
        .revoke_session(&session_token_digest(&tokens[0]))
        .await
        .unwrap();
    private(
        &call(&app, &path, &cookies[0]).await,
        StatusCode::UNAUTHORIZED,
    );
}
