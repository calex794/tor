//! FLOW: exact selected-book dates, private HTTP and current public metadata.
//! Temporal analytics are explicit legacy fixtures; Chromium proves C progress adds no capture.
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const TODAY: &str = "(now() AT TIME ZONE 'UTC')::date";

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

fn ids(values: &[i64]) -> String {
    values
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

struct Fixtures {
    url: String,
    users: Vec<i64>,
    books: Vec<i64>,
    coverage: Vec<i64>,
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        if !self.books.is_empty() {
            let books = ids(&self.books);
            pg(
                &self.url,
                &format!(
                    "BEGIN; DELETE FROM rec_quality_daily WHERE book_id IN({books});
                 DELETE FROM organic_discovery_quality_daily WHERE book_id IN({books});
                 DELETE FROM rec_dirty_buckets WHERE book_id IN({books});
                 DELETE FROM organic_discovery_dirty WHERE book_id IN({books});
                 DELETE FROM books WHERE id IN({books}); COMMIT"
                ),
            );
        }
        if !self.users.is_empty() {
            let users = ids(&self.users);
            pg(
                &self.url,
                &format!(
                    "BEGIN; DELETE FROM account_roles WHERE user_id IN({users});
                 DELETE FROM users WHERE id IN({users}); COMMIT"
                ),
            );
        }
        if !self.coverage.is_empty() {
            pg(
                &self.url,
                &format!(
                    "DELETE FROM reader_book_return_coverage WHERE id IN({})",
                    ids(&self.coverage)
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
        writeln!(input, "BEGIN; LOCK TABLE reader_book_return_subjects IN ACCESS EXCLUSIVE MODE; SELECT 'book-returns-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "book-returns-locked");
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
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
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

fn period<'a>(html: &'a str, key: &str) -> &'a str {
    let marker = format!("data-testid=\"audience-book-returns-{key}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("selected-book period")
        .split("</section>")
        .next()
        .unwrap()
}

fn horizon(html: &str, day: i32) -> &str {
    let marker = format!("data-return-day=\"{day}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("return horizon")
        .split("</article>")
        .next()
        .unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Counts {
    enrolled: i64,
    /// eligible, returned, pending, unknown; in D7/D30 order.
    days: [[i64; 4]; 2],
}

fn counts(html: &str, key: &str) -> Counts {
    let section = period(html, key);
    assert_eq!(section.matches("data-return-day=").count(), 2);
    let days = [7, 30].map(|day| {
        let row = horizon(section, day);
        let eligible = metric(row, "eligible").parse::<i64>().unwrap();
        let returned = if eligible == 0 {
            assert_eq!(metric(row, "returned"), "—", "no fabricated zero");
            assert_eq!(metric(row, "rate"), "Пока не рассчитана");
            0
        } else {
            let returned = metric(row, "returned").parse::<i64>().unwrap();
            assert_eq!(
                metric(row, "rate"),
                format!("{:.1}%", 100.0 * returned as f64 / eligible as f64).replace('.', ",")
            );
            returned
        };
        [
            eligible,
            returned,
            metric(row, "pending").parse().unwrap(),
            metric(row, "unknown").parse().unwrap(),
        ]
    });
    let enrolled = metric(section, "cohort").parse().unwrap();
    for day in days {
        assert_eq!(day[0] + day[2] + day[3], enrolled);
        assert!(day[1] <= day[0]);
    }
    if enrolled == 0 {
        assert!(section.contains("audience-book-returns-empty"));
    }
    Counts { enrolled, days }
}

async fn report(app: &Router, cookie: &str, book: i64, days: i32) -> String {
    let path = format!("/admin/audience/returns?book_id={book}&days={days}");
    let response = call(app, &path, cookie).await;
    private(&response, StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("data-testid=\"collection-retired\""));
    assert!(html.contains("data-current-measurement=\"unavailable\">Не измеряется"));
    assert!(html.contains("data-measurement=\"legacy-consented\""));
    html
}

fn date_bounds(url: &str, html: &str, days: i32) {
    for (key, start, end) in [
        ("current", days - 1, -1),
        ("previous", 2 * days - 1, days - 1),
    ] {
        let text = period(html, key);
        for age in [start, end] {
            let date = pg(url, &format!("SELECT ({TODAY}-({age}))::text"));
            assert!(text.contains(&date), "{key}: {date} UTC boundary");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn book_returns_http_exact_days_isolation_visibility_and_live_admin() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("AudienceBookReturnsPassword2026").unwrap();
    let mut fixtures = Fixtures {
        url: url.clone(),
        users: Vec::new(),
        books: Vec::new(),
        coverage: Vec::new(),
    };
    let mut cookies = Vec::new();
    for kind in ["admin", "reader", "moderator"] {
        let token = generate_session_token();
        fixtures.users.push(
            accounts
                .register_with_session(
                    &format!("flow-book-returns-{kind}-{stamp}@example.invalid"),
                    "Book returns HTTP fixture",
                    &password,
                    &session_token_digest(&token),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={token}"));
    }
    let (admin, moderator) = (fixtures.users[0], fixtures.users[2]);
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"));
    let genre = format!("book-returns-{stamp}");
    for (index, title) in ["<script>alert(1)</script> & книга", "OTHER-BOOK-SENTINEL"]
        .into_iter()
        .enumerate()
    {
        fixtures.books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)
             VALUES('ci:flow:book-returns:http:{stamp}:{index}','ru','{title}','public','basic',ARRAY['{genre}']) RETURNING id"
        )).parse().unwrap());
    }
    let (book, other) = (fixtures.books[0], fixtures.books[1]);
    let path = format!("/admin/audience/returns?book_id={book}&days=90");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        "http://127.0.0.1:3098".into(),
    );
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let denied = call(
            &app,
            "/admin/audience/returns?book_id=bad&days=bad&subject_id=forged",
            cookie,
        )
        .await;
        private(&denied, status);
        assert!(!body(denied).await.contains("data-metric="));
    }
    for query in [
        "book_id=",
        "book_id=0",
        "book_id=-1",
        "book_id=abc",
        "book_id=1.0",
        "book_id=9223372036854775808",
        "book_id=1&book_id=2",
        "book_id=1&days=0",
        "book_id=1&days=2",
        "book_id=1&days=-1",
        "book_id=1&days=365",
        "book_id=1&days=2147483648",
        "book_id=1&days=bad",
        "book_id=1&days=7&days=30",
        "book_id=1&user_id=1",
        "book_id=1&subject_id=forged",
    ] {
        let invalid = call(
            &app,
            &format!("/admin/audience/returns?{query}"),
            &cookies[0],
        )
        .await;
        private(&invalid, StatusCode::BAD_REQUEST);
        assert!(!body(invalid).await.contains("data-metric="));
    }
    let missing = call(
        &app,
        "/admin/audience/returns?book_id=9223372036854775807",
        &cookies[0],
    )
    .await;
    private(&missing, StatusCode::NOT_FOUND);
    assert!(!body(missing).await.contains("data-metric="));

    let unobserved = report(&app, &cookies[0], book, 90).await;
    assert!(unobserved.contains("Возвращаются ли к этой книге"));
    assert!(unobserved.contains("data-testid=\"audience-book-returns-title\""));
    assert!(unobserved.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; книга"));
    assert!(!unobserved.contains("<script>alert(1)"));
    if pg(&url, "SELECT count(*) FROM reader_book_return_coverage") == "0" {
        assert!(unobserved.contains("audience-book-returns-not-started"));
        assert!(!unobserved.contains("data-metric="));
    }
    // Closed intervals leave a half-day hole and an entirely unobserved day.
    // Historical values are explicit report fixtures, never production backfill.
    for (start, end) in [
        (200, "(date_trunc('day',now() AT TIME ZONE 'UTC')-interval '10 days'+interval '12 hours') AT TIME ZONE 'UTC'"),
        (8, "now()"),
    ] {
        fixtures.coverage.push(pg(&url, &format!(
            "INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at,ended_at)
             VALUES(({TODAY}-{start})::timestamp AT TIME ZONE 'UTC',{end},{end}) RETURNING id"
        )).parse().unwrap());
    }
    let empty = report(&app, &cookies[0], book, 90).await;
    for key in ["current", "previous"] {
        assert_eq!(
            counts(&empty, key),
            Counts {
                enrolled: 0,
                days: [[0; 4]; 2]
            }
        );
    }
    // Today's D7/D30 are not mature, even with a positive flag. Returns in a
    // later fully covered target day remain valid despite an earlier gap.
    for (index, (age, flags)) in [
        (0, [false, false]),
        (1, [false, false]),
        (7, [true, false]),
        (8, [true, false]),
        (8, [false, false]),
        (10, [true, false]),
        (16, [true, false]),
        (17, [true, false]),
        (30, [false, true]),
        (31, [false, true]),
    ]
    .into_iter()
    .enumerate()
    {
        let user = pg(&url, &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('flow-book-return-observation-{stamp}-{index}@example.invalid','Private observation','$argon2id$fixture') RETURNING id"
        )).parse::<i64>().unwrap();
        fixtures.users.push(user);
        pg(&url, &format!(
            "WITH consent AS(INSERT INTO rec_subjects(user_id,enabled,enabled_at)
             VALUES({user},true,({TODAY}-{age})::timestamp AT TIME ZONE 'UTC') RETURNING subject_id)
             INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day,returned_d7,returned_d30)
             SELECT subject_id,{book},{TODAY}-{age},{},{} FROM consent", flags[0], flags[1]
        ));
    }
    for (age, flags) in [
        (45, [0, 1]),
        (89, [1, 0]),
        (90, [1, 1]),
        (179, [0, 0]),
        (180, [1, 1]),
    ] {
        for (day, returned) in [7, 30].into_iter().zip(flags) {
            pg(&url, &format!(
                "INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown)
                 VALUES({book},{TODAY}-{age},{day},1,1,{returned},0)"
            ));
        }
    }
    let same_reader = fixtures.users[3];
    pg(&url, &format!(
        "INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day)
         SELECT subject_id,{other},{TODAY} FROM rec_subjects WHERE user_id={same_reader};
         INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown)
         SELECT {other},{TODAY}-45,day,987654,987654,12345,0 FROM unnest(ARRAY[7,30]) day"
    ));
    let subjects = pg(
        &url,
        &format!(
            "SELECT string_agg(subject_id::text,',') FROM rec_subjects WHERE user_id IN({})",
            ids(&fixtures.users)
        ),
    );
    let pending = Counts {
        enrolled: 1,
        days: [[0, 0, 1, 0]; 2],
    };
    for (days, expected) in [
        (1, [pending, pending]),
        (
            7,
            [
                Counts {
                    enrolled: 2,
                    days: [[0, 0, 2, 0]; 2],
                },
                Counts {
                    enrolled: 4,
                    days: [[3, 2, 0, 1], [0, 0, 4, 0]],
                },
            ],
        ),
        (
            30,
            [
                Counts {
                    enrolled: 8,
                    days: [[3, 2, 2, 3], [0, 0, 8, 0]],
                },
                Counts {
                    enrolled: 3,
                    days: [[3, 0, 0, 0], [2, 2, 0, 1]],
                },
            ],
        ),
        (
            90,
            [
                Counts {
                    enrolled: 12,
                    days: [[7, 3, 2, 3], [3, 2, 8, 1]],
                },
                Counts {
                    enrolled: 2,
                    days: [[2, 1, 0, 0]; 2],
                },
            ],
        ),
    ] {
        let html = report(&app, &cookies[0], book, days).await;
        date_bounds(&url, &html, days);
        for (key, expected) in ["current", "previous"].into_iter().zip(expected) {
            assert_eq!(counts(&html, key), expected, "{days} days, {key}");
        }
        for forbidden in [
            "OTHER-BOOK-SENTINEL".to_owned(),
            "987654".to_owned(),
            format!("flow-book-returns-admin-{stamp}@example.invalid"),
            cookies[0].trim_start_matches("tor-dev-session=").to_owned(),
        ] {
            assert!(
                !html.contains(&forbidden),
                "no unrelated/personal data: {forbidden}"
            );
        }
        for subject in subjects.split(',') {
            assert!(
                !html.contains(subject),
                "consent epochs are never disclosed"
            );
        }
        assert!(html.contains("audience-book-returns-partial"));
        assert!(html.contains("audience-book-returns-collection-gap"));
        assert!(html.contains("Недостаточно завершённых сопоставимых наблюдений"));
        let repeat = report(&app, &cookies[0], book, days).await;
        for key in ["current", "previous"] {
            assert_eq!(
                counts(&repeat, key),
                counts(&html, key),
                "GET cannot collect"
            );
        }
    }
    let independent = report(&app, &cookies[0], other, 90).await;
    assert_eq!(
        counts(&independent, "current"),
        Counts {
            enrolled: 987655,
            days: [[987654, 12345, 1, 0]; 2],
        }
    );
    assert!(!independent.contains("alert(1)"));
    let default = call(
        &app,
        &format!("/admin/audience/returns?book_id={book}"),
        &cookies[0],
    )
    .await;
    private(&default, StatusCode::OK);
    let default = body(default).await;
    let ninety = report(&app, &cookies[0], book, 90).await;
    for key in ["current", "previous"] {
        assert_eq!(counts(&default, key), counts(&ninety, key));
    }
    assert!(default.contains(&format!("name=\"book_id\" value=\"{book}\"")));
    let global = body(call(&app, "/admin/audience/returns?days=90", &cookies[0]).await).await;
    assert!(global.contains("data-testid=\"audience-returns\""));
    assert!(!global.contains("audience-book-returns-title"));
    assert!(!global.contains("alert(1)"));

    // The public list is the real entry point; it preserves the selected period.
    let list = call(
        &app,
        &format!("/admin/audience/books?days=30&genre={genre}"),
        &cookies[0],
    )
    .await;
    if list.status() != StatusCode::OK {
        let status = list.status();
        let preview = body(list).await.chars().take(512).collect::<String>();
        panic!("books entry returned {status} for fixture genre {genre}: {preview}");
    }
    private(&list, StatusCode::OK);
    let list = body(list).await;
    let links = list
        .split('<')
        .filter(|tag| tag.contains("data-testid=\"audience-book-returns-link\""))
        .collect::<Vec<_>>();
    assert_eq!(links.len(), 2);
    for selected in [book, other] {
        assert!(links
            .iter()
            .any(|link| link.contains(&format!("book_id={selected}")) && link.contains("days=30")));
    }

    // A complete coverage fixture makes the comparison measurable. Pending
    // observations stay out of its denominator, including the current target day.
    fixtures.coverage.push(
        pg(
            &url,
            &format!(
                "INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at)
         VALUES(({TODAY}-200)::timestamp AT TIME ZONE 'UTC',now()) RETURNING id"
            ),
        )
        .parse()
        .unwrap(),
    );
    let healthy = report(&app, &cookies[0], book, 90).await;
    assert_eq!(
        counts(&healthy, "current"),
        Counts {
            enrolled: 12,
            days: [[9, 5, 3, 0], [3, 2, 9, 0]]
        }
    );
    assert!(healthy.contains("+5,6 процентного пункта"));
    assert!(healthy.contains("+16,7 процентного пункта"));
    assert!(!healthy.contains("audience-book-returns-collection-gap"));

    pg(
        &url,
        &format!("UPDATE books SET title='Название после правки & книга' WHERE id={book}"),
    );
    let renamed = report(&app, &cookies[0], book, 90).await;
    assert!(renamed.contains("Название после правки &amp; книга"));
    assert!(!renamed.contains("alert(1)"));
    for changed in [
        "visibility='draft'",
        "visibility_status='hidden'",
        "visibility_status='blocked'",
        "visibility_status='pending_moderation'",
        "admin_hold=true",
        "is_deleted=true",
        "lifecycle_status='pending_deletion'",
        "access_level='adult'",
        "access_level='pro_adult'",
        "access_level=NULL",
    ] {
        pg(&url, &format!("UPDATE books SET {changed} WHERE id={book}"));
        let hidden = call(&app, &path, &cookies[0]).await;
        private(&hidden, StatusCode::NOT_FOUND);
        let hidden = body(hidden).await;
        assert!(
            !hidden.contains("Название после правки"),
            "live exclusion: {changed}"
        );
        assert!(!hidden.contains("data-metric="));
        pg(&url, &format!("UPDATE books SET visibility='public',visibility_status='active',admin_hold=false,is_deleted=false,lifecycle_status='in_progress',access_level='basic' WHERE id={book}"));
    }
    for audience in ["adult", "unreviewed"] {
        pg(&url, &format!("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES({book},'{audience}')"));
        let hidden = call(&app, &path, &cookies[0]).await;
        private(&hidden, StatusCode::NOT_FOUND);
        let hidden = body(hidden).await;
        assert!(!hidden.contains("Название после правки"));
        assert!(!hidden.contains("data-metric="));
        pg(
            &url,
            &format!("DELETE FROM book_recommendation_audiences WHERE book_id={book}"),
        );
    }
    let restored = report(&app, &cookies[0], book, 90).await;
    assert_eq!(counts(&restored, "current"), counts(&healthy, "current"));
    let post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::COOKIE, &cookies[0])
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        post.status(),
        StatusCode::FORBIDDEN | StatusCode::METHOD_NOT_ALLOWED
    ));
    assert!(!body(post).await.contains("data-metric="));
    let export = call(
        &app,
        &format!("/admin/audience/returns/export?book_id={book}"),
        &cookies[0],
    )
    .await;
    assert!(!export.status().is_success());
    assert!(!body(export).await.contains("data-metric="));

    let lock = LockedReport::acquire(&url);
    let started = Instant::now();
    let unavailable = call(&app, &path, &cookies[0]).await;
    private(&unavailable, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "bounded SQL/HTTP failure"
    );
    let unavailable = body(unavailable).await;
    assert!(unavailable.contains("audience-book-returns-unavailable"));
    assert!(!unavailable.contains("data-metric="));
    assert!(!unavailable.contains("Название после правки"));
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let revoked = call(&app, &path, &cookies[0]).await;
    private(&revoked, StatusCode::FORBIDDEN);
    assert!(!body(revoked).await.contains("data-metric="));
    drop(lock);
    pg(&url, &format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
    assert_eq!(
        counts(&report(&app, &cookies[0], book, 90).await, "current"),
        counts(&healthy, "current")
    );
}
