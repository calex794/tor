//! FLOW: complete current genre/tag book observations through the private HTTP path.
//! Temporal analytics are explicit legacy fixtures; Chromium proves C progress adds no capture.
#![forbid(unsafe_code)]
#[path = "support/legacy_analytics.rs"]
mod legacy_analytics;

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
    assert!(url.ends_with("/tor_ci_legacy_analytics") && url.contains("127.0.0.1:5432"));
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
    let mut child = client(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{sql};").unwrap();
    let output = child.wait_with_output().unwrap();
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
        writeln!(input, "BEGIN; LOCK TABLE reader_book_return_subjects IN ACCESS EXCLUSIVE MODE; SELECT 'group-returns-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "group-returns-locked");
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

fn after<'a>(html: &'a str, marker: &str) -> &'a str {
    html.split_once(marker).expect(marker).1
}

fn value(html: &str) -> &str {
    after(html, ">").split('<').next().unwrap().trim()
}

fn metric<'a>(html: &'a str, key: &str) -> &'a str {
    value(after(html, &format!("data-metric=\"{key}\"")))
}

fn period<'a>(html: &'a str, key: &str) -> &'a str {
    after(
        html,
        &format!("data-testid=\"audience-group-returns-{key}\""),
    )
    .split("</section>")
    .next()
    .unwrap()
}

fn horizon(html: &str, day: i32) -> &str {
    after(html, &format!("data-return-day=\"{day}\""))
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
        assert!(section.contains("audience-group-returns-empty"));
    }
    Counts { enrolled, days }
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

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn link(html: &str, testid: &str) -> String {
    let marker = format!("data-testid=\"{testid}\"");
    let tag = html
        .split('<')
        .find(|tag| tag.contains(&marker))
        .expect("navigation link");
    after(tag, "href=\"")
        .split('"')
        .next()
        .unwrap()
        .replace("&amp;", "&")
}

fn book_count(html: &str) -> i64 {
    value(after(
        html,
        "data-testid=\"audience-group-returns-book-count\"",
    ))
    .parse()
    .unwrap()
}

fn group_path(kind: &str, label: Option<&str>, days: i32) -> String {
    let filter = label.map_or_else(
        || "without_label=true".into(),
        |label| format!("label={}", encoded(label)),
    );
    format!("/admin/audience/returns?kind={kind}&{filter}&days={days}")
}

async fn report(app: &Router, cookie: &str, kind: &str, label: Option<&str>, days: i32) -> String {
    let html = get(app, &group_path(kind, label, days), cookie).await;
    assert!(html.contains("Возвращения к книгам группы"));
    assert!(html.contains("data-testid=\"audience-group-returns\""));
    assert!(html.contains("data-testid=\"collection-retired\""));
    assert!(html.contains("data-current-measurement=\"unavailable\">Не измеряется"));
    assert!(html.contains("data-measurement=\"legacy-consented\""));
    html
}

async fn get(app: &Router, path: &str, cookie: &str) -> String {
    let response = call(app, path, cookie).await;
    private(&response, StatusCode::OK);
    body(response).await
}

fn date_bounds(url: &str, html: &str, days: i32) {
    for (key, start, end) in [
        ("current", days - 1, -1),
        ("previous", 2 * days - 1, days - 1),
    ] {
        for age in [start, end] {
            let date = pg(url, &format!("SELECT ({TODAY}-({age}))::text"));
            assert!(
                period(html, key).contains(&date),
                "{key}: {date} UTC boundary"
            );
        }
    }
}

const ZERO: Counts = Counts {
    enrolled: 0,
    days: [[0; 4]; 2],
};
const PENDING: [i64; 4] = [0, 0, 1, 0];
const UNKNOWN: [i64; 4] = [0, 0, 0, 1];

fn add(mut left: Counts, right: Counts) -> Counts {
    left.enrolled += right.enrolled;
    for (a, b) in left.days.iter_mut().zip(right.days) {
        for (a, b) in a.iter_mut().zip(b) {
            *a += b;
        }
    }
    left
}

#[derive(Clone, Copy)]
struct Fact {
    book: i64,
    age: i32,
    counts: Counts,
}

// A small independent arithmetic oracle over explicit fixtures. It knows no SQL,
// metadata joins or report repository and cannot accidentally inherit a page cap.
fn expected(facts: &[Fact], books: &[i64], days: i32, previous: bool) -> Counts {
    let first_age = if previous { days } else { 0 };
    facts
        .iter()
        .filter(|fact| {
            books.contains(&fact.book) && (first_age..first_age + days).contains(&fact.age)
        })
        .fold(ZERO, |total, fact| add(total, fact.counts))
}

fn exact(html: &str, facts: &[Fact], books: &[i64], days: i32) {
    for (key, previous) in [("current", false), ("previous", true)] {
        assert_eq!(
            counts(html, key),
            expected(facts, books, days, previous),
            "{days} {key}"
        );
    }
}

fn archived(book: i64, age: i32, enrolled: i64, returned: [i64; 2]) -> Fact {
    Fact {
        book,
        age,
        counts: Counts {
            enrolled,
            days: returned.map(|value| [enrolled, value, 0, 0]),
        },
    }
}

fn create_book(fixtures: &mut Fixtures, stamp: u128, key: &str, genres: &str, tags: &str) -> i64 {
    let book = pg(&fixtures.url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
         VALUES('ci:flow:group-returns:http:{stamp}:{key}','ru','Private fixture title {key}',
         'public','basic',{genres},{tags}) RETURNING id"
    )).parse().unwrap();
    fixtures.books.push(book);
    book
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit legacy analytics baseline 191"]
async fn group_returns_http_exact_periods_complete_sums_current_metadata_and_private_access() {
    let url = legacy_analytics::url();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("AudienceGroupReturnsPassword2026").unwrap();
    let mut fixtures = Fixtures {
        url: url.clone(),
        users: Vec::new(),
        books: Vec::new(),
        coverage: Vec::new(),
    };
    let mut cookies = Vec::new();
    for role in ["admin", "reader", "moderator"] {
        let token = generate_session_token();
        fixtures.users.push(
            accounts
                .register_with_session(
                    &format!("flow-group-returns-{role}-{stamp}@example.invalid"),
                    "Group returns HTTP fixture",
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
    let prefix = format!("!r{stamp}");
    let label = format!("{prefix} &+\"<");
    let overlap = format!("{prefix}-overlap");
    let tag = format!("{prefix}-tag");
    let other_tag = format!("{prefix}-other");
    assert!(label.chars().count() < 40);
    let first = create_book(
        &mut fixtures,
        stamp,
        "first",
        &format!("ARRAY['{label}','{overlap}','{label}']"),
        &format!("ARRAY['{tag}','{other_tag}','{tag}']"),
    );
    let second = create_book(
        &mut fixtures,
        stamp,
        "second",
        &format!("ARRAY['{label}']"),
        &format!("ARRAY['{tag}']"),
    );
    let unrelated = create_book(
        &mut fixtures,
        stamp,
        "unrelated",
        &format!("ARRAY['{overlap}']"),
        &format!("ARRAY['{label}']"),
    );
    let mut selected = vec![first, second];
    for index in 2..25 {
        selected.push(create_book(
            &mut fixtures,
            stamp,
            &format!("bulk-{index}"),
            &format!("ARRAY['{label}']"),
            &format!("ARRAY['{other_tag}']"),
        ));
    }
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        "http://127.0.0.1:3098".into(),
    );
    let path = group_path("genres", Some(&label), 90);
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let denied = call(
            &app,
            "/admin/audience/returns?kind=bad&label=&days=bad&subject_id=forged",
            cookie,
        )
        .await;
        private(&denied, status);
        assert!(!body(denied).await.contains("data-metric="));
    }
    let mut invalid = vec![
        "kind=genres",
        "kind=tags&without_label=false",
        "label=x",
        "without_label=true",
        "kind=genres&label=",
        "kind=genres&label=%20%20",
        "kind=genres&label=%00",
        "kind=genres&label=%09",
        "kind=genres&label=%7F",
        "kind=authors&label=x",
        "kind=Genres&label=x",
        "kind=genres&label=x&days=2",
        "kind=genres&label=x&days=bad",
        "kind=genres&label=x&days=7&days=30",
        "kind=genres&kind=genres&label=x",
        "kind=genres&label=x&label=x",
        "kind=genres&label=x&without_label=true",
        "kind=genres&without_label=1",
        "kind=genres&without_label=TRUE",
        "kind=genres&without_label=true&without_label=true",
        "kind=genres&label=x&user_id=1",
        "kind=genres&label=x&subject_id=forged",
        "kind=genres&label=x&after=anything",
        "book_id=1&kind=genres",
        "book_id=1&label=x",
        "book_id=1&without_label=false",
        "book_id=1&kind=tags&without_label=true",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    invalid.push(format!("kind=genres&label={}", encoded(&"я".repeat(41))));
    invalid.push(format!(
        "kind=genres&label={}",
        encoded(&format!("{}x", " ".repeat(512)))
    ));
    for query in invalid {
        let response = call(
            &app,
            &format!("/admin/audience/returns?{query}"),
            &cookies[0],
        )
        .await;
        private(&response, StatusCode::BAD_REQUEST);
        assert!(
            !body(response).await.contains("data-metric="),
            "invalid query: {query}"
        );
    }
    let unobserved = report(&app, &cookies[0], "genres", Some(&label), 90).await;
    assert_eq!(book_count(&unobserved), 25);
    assert!(unobserved.contains(&escaped(&label)));
    assert!(!unobserved.contains(&label));
    if pg(&url, "SELECT count(*) FROM reader_book_return_coverage") == "0" {
        assert!(unobserved.contains("audience-group-returns-not-started"));
        assert!(!unobserved.contains("data-metric="));
    }
    // Closed historical coverage leaves a half day and a full day unobserved.
    // These are explicit HTTP fixtures, not an invented browser return.
    for (start, end) in [(200, "(date_trunc('day',now() AT TIME ZONE 'UTC')-interval '10 days'+interval '12 hours') AT TIME ZONE 'UTC'"), (8, "now()") ] {
        fixtures.coverage.push(pg(&url, &format!(
            "INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at,ended_at)
             VALUES(({TODAY}-{start})::timestamp AT TIME ZONE 'UTC',{end},{end}) RETURNING id"
        )).parse().unwrap());
    }
    let empty_label = format!("{prefix}-empty");
    let empty = report(&app, &cookies[0], "genres", Some(&empty_label), 90).await;
    assert_eq!(book_count(&empty), 0);
    assert!(empty.contains("audience-group-returns-no-books"));
    assert_eq!(counts(&empty, "current"), ZERO);
    assert_eq!(counts(&empty, "previous"), ZERO);
    let mut facts = Vec::new();
    let live = [
        (0, [false, false], [PENDING, PENDING]),
        (1, [false, false], [PENDING, PENDING]),
        (7, [true, false], [UNKNOWN, PENDING]),
        (8, [true, false], [[1, 1, 0, 0], PENDING]),
        (8, [false, false], [[1, 0, 0, 0], PENDING]),
        (10, [true, false], [[1, 1, 0, 0], PENDING]),
        (16, [true, false], [UNKNOWN, PENDING]),
        (17, [true, false], [UNKNOWN, PENDING]),
        (30, [false, true], [[1, 0, 0, 0], UNKNOWN]),
        (31, [false, true], [[1, 0, 0, 0], [1, 1, 0, 0]]),
    ];
    for (index, (age, flags, days)) in live.into_iter().enumerate() {
        let user = pg(&url, &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('flow-group-return-observation-{stamp}-{index}@example.invalid','Private observation','$argon2id$fixture') RETURNING id"
        )).parse::<i64>().unwrap();
        fixtures.users.push(user);
        pg(&url, &format!(
            "WITH consent AS(INSERT INTO rec_subjects(user_id,enabled,enabled_at)
             VALUES({user},true,({TODAY}-{age})::timestamp AT TIME ZONE 'UTC') RETURNING subject_id)
             INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day,returned_d7,returned_d30)
             SELECT subject_id,{first},{TODAY}-{age},{},{} FROM consent", flags[0], flags[1]));
        facts.push(Fact {
            book: first,
            age,
            counts: Counts { enrolled: 1, days },
        });
    }
    // One account reading both books contributes two book observations, not one person.
    let same_reader = fixtures.users[3];
    pg(
        &url,
        &format!(
            "INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day)
        SELECT subject_id,{second},{TODAY} FROM rec_subjects WHERE user_id={same_reader}"
        ),
    );
    facts.push(Fact {
        book: second,
        age: 0,
        counts: Counts {
            enrolled: 1,
            days: [PENDING; 2],
        },
    });
    for (age, returned) in [
        (45, [0, 1]),
        (89, [1, 0]),
        (90, [1, 1]),
        (179, [0, 0]),
        (180, [1, 1]),
    ] {
        facts.push(archived(first, age, 1, returned));
    }
    facts.push(archived(second, 45, 9, [0, 9]));
    facts.push(archived(unrelated, 45, 987654, [12345, 654321]));
    for (index, book) in selected.iter().enumerate().skip(2) {
        for age in 45..180 {
            facts.push(archived(
                *book,
                age,
                index as i64,
                [i64::from(index % 2 == 0), index as i64 - 1],
            ));
        }
    }
    let daily = facts
        .iter()
        .filter(|fact| fact.age >= 45)
        .flat_map(|fact| {
            [7, 30]
                .into_iter()
                .zip(fact.counts.days)
                .map(move |(day, values)| {
                    format!(
                        "({},{TODAY}-{}, {day},{},{},{},{})",
                        fact.book, fact.age, fact.counts.enrolled, values[0], values[1], values[3]
                    )
                })
        })
        .collect::<Vec<_>>()
        .join(",");
    pg(&url, &format!("INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown) VALUES {daily}"));
    assert!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_book_return_daily WHERE book_id IN({})",
                ids(&selected)
            )
        )
        .parse::<i64>()
        .unwrap()
            > 100
    );
    let subjects = pg(
        &url,
        &format!(
            "SELECT string_agg(subject_id::text,',') FROM rec_subjects WHERE user_id IN({})",
            ids(&fixtures.users)
        ),
    );
    for days in [1, 7, 30, 90] {
        for (kind, selected_label, members) in [
            ("genres", &label, selected.clone()),
            ("tags", &tag, vec![first, second]),
            ("genres", &overlap, vec![first, unrelated]),
            ("tags", &label, vec![unrelated]),
        ] {
            let html = report(&app, &cookies[0], kind, Some(selected_label), days).await;
            assert_eq!(book_count(&html), members.len() as i64);
            date_bounds(&url, &html, days);
            exact(&html, &facts, &members, days);
        }
        let html = report(&app, &cookies[0], "genres", Some(&label), days).await;
        assert!(html.contains("audience-group-returns-partial"));
        assert!(html.contains("audience-group-returns-collection-gap"));
        assert!(html.contains("Недостаточно завершённых сопоставимых наблюдений"));
        for secret in [
            "Private fixture title".to_owned(),
            "987654".to_owned(),
            format!("flow-group-returns-admin-{stamp}@example.invalid"),
            cookies[0].trim_start_matches("tor-dev-session=").to_owned(),
        ] {
            assert!(
                !html.contains(&secret),
                "no unrelated or personal data: {secret}"
            );
        }
        for subject in subjects.split(',') {
            assert!(!html.contains(subject));
        }
        let repeat = report(&app, &cookies[0], "genres", Some(&label), days).await;
        for key in ["current", "previous"] {
            assert_eq!(
                counts(&repeat, key),
                counts(&html, key),
                "GET cannot collect"
            );
        }
    }
    // Percentages use combined observations. The first book and second book have
    // very different weights; averaging their separately rounded percentages is wrong.
    let weighted = report(&app, &cookies[0], "tags", Some(&tag), 90).await;
    assert_eq!(
        counts(&weighted, "current"),
        Counts {
            enrolled: 22,
            days: [[16, 3, 3, 3], [12, 11, 9, 1]]
        }
    );
    assert_eq!(
        metric(horizon(period(&weighted, "current"), 7), "rate"),
        "18,8%"
    );
    assert_eq!(
        metric(horizon(period(&weighted, "current"), 30), "rate"),
        "91,7%"
    );
    let canonical = report(
        &app,
        &cookies[0],
        "genres",
        Some(&format!("  {}  ", label.to_uppercase())),
        90,
    )
    .await;
    assert_eq!(book_count(&canonical), 25);
    assert!(canonical.contains(&format!("name=\"label\" value=\"{}\"", escaped(&label))));
    assert!(canonical.contains("name=\"kind\" value=\"genres\""));
    let default = get(&app, &path.replace("&days=90", ""), &cookies[0]).await;
    let neutral = get(&app, &format!("{path}&without_label=false"), &cookies[0]).await;
    for key in ["current", "previous"] {
        assert_eq!(counts(&default, key), counts(&canonical, key));
        assert_eq!(counts(&neutral, key), counts(&canonical, key));
    }
    // Navigation carries the exact escaped label and selected period in both directions.
    let groups = get(
        &app,
        &format!(
            "/admin/audience/groups?kind=genres&days=30&after={}",
            encoded(&prefix)
        ),
        &cookies[0],
    )
    .await;
    let row = groups
        .split("data-testid=\"audience-group-row\"")
        .skip(1)
        .find(|row| {
            row.split('>')
                .next()
                .unwrap()
                .contains(&format!("data-label=\"{}\"", escaped(&label)))
        })
        .unwrap();
    let entry = link(row, "audience-group-returns-link");
    assert!(
        entry.contains(&format!("label={}", encoded(&label)))
            && entry.contains("kind=genres")
            && entry.contains("days=30")
    );
    let detail = get(&app, &entry, &cookies[0]).await;
    exact(&detail, &facts, &selected, 30);
    let books_link = link(&detail, "audience-group-returns-books");
    assert!(
        books_link.contains(&format!("genre={}", encoded(&label)))
            && books_link.contains("days=30")
    );
    let books = get(&app, &books_link, &cookies[0]).await;
    assert_eq!(
        books.matches("data-testid=\"audience-book-row\"").count(),
        20
    );
    let later = get(&app, &link(&books, "audience-books-next"), &cookies[0]).await;
    assert_eq!(
        later.matches("data-testid=\"audience-book-row\"").count(),
        5
    );
    assert_eq!(
        book_count(&detail),
        25,
        "group totals are independent of the books page"
    );
    // Groups with an absent label include only genuinely unlabelled books.
    for (kind, field, title) in [
        ("genres", "without_genre", "Без жанра"),
        ("tags", "without_tag", "Без тега"),
    ] {
        let before = report(&app, &cookies[0], kind, None, 90).await;
        let missing = create_book(&mut fixtures, stamp, kind, "'{}'", "'{}'");
        pg(&url, &format!("INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown)
            SELECT {missing},{TODAY}-45,day,3,3,2,0 FROM unnest(ARRAY[7,30]) day"));
        let after = report(&app, &cookies[0], kind, None, 90).await;
        assert_eq!(book_count(&after), book_count(&before) + 1);
        assert_eq!(
            counts(&after, "current"),
            add(
                counts(&before, "current"),
                archived(missing, 45, 3, [2; 2]).counts
            )
        );
        assert!(after.contains(title));
        assert!(after.contains("name=\"without_label\" value=\"true\""));
        assert!(link(&after, "audience-group-returns-books").contains(&format!("{field}=true")));
        assert!(!after.contains("name=\"label\""));
    }
    // Healthy complete coverage makes both comparisons meaningful; today's exact
    // target day still waits rather than becoming a measured return.
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
    for fact in &mut facts {
        if fact.book == first {
            match fact.age {
                7 => fact.counts.days[0] = PENDING,
                16 | 17 => fact.counts.days[0] = [1, 1, 0, 0],
                30 => fact.counts.days[1] = PENDING,
                _ => {}
            }
        }
    }
    let healthy = report(&app, &cookies[0], "genres", Some(&label), 90).await;
    exact(&healthy, &facts, &selected, 90);
    assert!(!healthy.contains("audience-group-returns-collection-gap"));
    assert!(!healthy.contains("Недостаточно завершённых сопоставимых наблюдений"));
    assert!(healthy.contains("процентного пункта"));
    let moved = format!("{prefix}-moved");
    pg(&url, &format!("UPDATE books SET genres=ARRAY['{moved}'],title='Changed private title' WHERE id={first}"));
    let changed = report(&app, &cookies[0], "genres", Some(&label), 90).await;
    assert_eq!(book_count(&changed), 24);
    exact(&changed, &facts, &selected[1..], 90);
    let destination = report(&app, &cookies[0], "genres", Some(&moved), 90).await;
    assert_eq!(book_count(&destination), 1);
    exact(&destination, &facts, &[first], 90);
    assert!(!destination.contains("Changed private title"));
    pg(
        &url,
        &format!("UPDATE books SET genres=ARRAY['{label}','{overlap}','{label}'] WHERE id={first}"),
    );
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
        pg(
            &url,
            &format!("UPDATE books SET {changed} WHERE id={first}"),
        );
        let hidden = report(&app, &cookies[0], "genres", Some(&label), 90).await;
        assert_eq!(book_count(&hidden), 24, "live exclusion: {changed}");
        exact(&hidden, &facts, &selected[1..], 90);
        pg(
            &url,
            &format!(
                "UPDATE books SET visibility='public',visibility_status='active',admin_hold=false,
            is_deleted=false,lifecycle_status='in_progress',access_level='basic' WHERE id={first}"
            ),
        );
    }
    for audience in ["adult", "unreviewed"] {
        pg(&url, &format!("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES({first},'{audience}')"));
        let hidden = report(&app, &cookies[0], "genres", Some(&label), 90).await;
        assert_eq!(book_count(&hidden), 24);
        exact(&hidden, &facts, &selected[1..], 90);
        pg(
            &url,
            &format!("DELETE FROM book_recommendation_audiences WHERE book_id={first}"),
        );
    }
    // Existing global and book selections are unchanged by the new branch.
    let book = get(
        &app,
        &format!("/admin/audience/returns?book_id={first}&days=90"),
        &cookies[0],
    )
    .await;
    let adapted = book.replace("audience-book-returns-", "audience-group-returns-");
    exact(&adapted, &facts, &[first], 90);
    let global = get(&app, "/admin/audience/returns?days=90", &cookies[0]).await;
    assert!(global.contains("data-testid=\"audience-returns\""));
    assert!(!global.contains("audience-group-returns-title"));
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
        "/admin/audience/returns/export?kind=genres&label=x",
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
        started.elapsed() < Duration::from_millis(1500),
        "950ms HTTP budget with scheduling margin"
    );
    let unavailable = body(unavailable).await;
    assert!(unavailable.contains("audience-group-returns-unavailable"));
    assert!(!unavailable.contains("data-metric="));
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let revoked = call(&app, &format!("{path}&subject_id=forged"), &cookies[0]).await;
    private(&revoked, StatusCode::FORBIDDEN);
    assert!(!body(revoked).await.contains("data-metric="));
    drop(lock);
    pg(&url, &format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
    let restored = report(&app, &cookies[0], "genres", Some(&label), 90).await;
    assert_eq!(counts(&restored, "current"), counts(&healthy, "current"));
}
