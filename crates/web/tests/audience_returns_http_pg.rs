//! FLOW: exact temporal fixtures exercise the real private HTTP/PG report.
//! Actual collection comes from C progress in the separate Chromium scenario.
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
const AGES: &str = "45,89,90,179,180";

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
    users: Vec<i64>,
    coverage: Vec<i64>,
    book: Option<i64>,
    saved_daily: Option<String>,
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved_daily {
            // The suite runs serially before starting the background collector.
            // Restore only the explicit fixture dates, including prior totals.
            pg(
                &self.url,
                &format!(
                    "BEGIN; DELETE FROM reader_return_daily WHERE cohort_day IN(
                 SELECT {TODAY}-age FROM unnest(ARRAY[{AGES}]) age);
                 INSERT INTO reader_return_daily SELECT * FROM jsonb_populate_recordset(
                 NULL::reader_return_daily,$saved${saved}$saved$::jsonb); COMMIT"
                ),
            );
        }
        if let Some(book) = self.book {
            pg(&self.url, &format!(
                "BEGIN; DELETE FROM outbox_events WHERE payload->>'book_id'='{book}';
                 DELETE FROM reader_book_positions WHERE book_id={book};
                 DELETE FROM reader_chapter_starts WHERE book_id={book};
                 DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id={book});
                 DELETE FROM books WHERE id={book}; COMMIT"
            ));
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
        if !self.coverage.is_empty() {
            let ids = self
                .coverage
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            pg(
                &self.url,
                &format!("DELETE FROM reader_return_coverage WHERE id IN({ids})"),
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
        writeln!(input, "BEGIN; LOCK TABLE reader_return_subjects IN ACCESS EXCLUSIVE MODE; SELECT 'returns-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "returns-locked");
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
    let marker = format!("data-testid=\"audience-returns-{key}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("period section")
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
    /// eligible, returned, pending, unknown; in D1/D7/D30 order.
    days: [[i64; 4]; 3],
}

fn counts(html: &str, key: &str) -> Counts {
    let section = period(html, key);
    let days = [1, 7, 30].map(|day| {
        let row = horizon(section, day);
        let eligible = metric(row, "eligible").parse::<i64>().unwrap();
        let returned = if eligible == 0 {
            assert_eq!(metric(row, "returned"), "—", "not a measured zero");
            assert_eq!(metric(row, "rate"), "Пока не рассчитана");
            0
        } else {
            let value = metric(row, "returned").parse::<i64>().unwrap();
            assert_eq!(
                metric(row, "rate"),
                format!("{:.1}%", 100.0 * value as f64 / eligible as f64).replace('.', ",")
            );
            value
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
        assert_eq!(
            day[0] + day[2] + day[3],
            enrolled,
            "every observed cohort is eligible, pending or unknown"
        );
        assert!(day[1] <= day[0]);
    }
    if enrolled == 0 {
        assert!(section.contains("audience-returns-empty"));
    }
    Counts { enrolled, days }
}

fn delta(before: Counts, after: Counts, expected: Counts) {
    assert_eq!(after.enrolled - before.enrolled, expected.enrolled);
    for (index, (before, after)) in before.days.into_iter().zip(after.days).enumerate() {
        assert_eq!(
            std::array::from_fn::<_, 4, _>(|column| after[column] - before[column]),
            expected.days[index],
            "horizon {}",
            [1, 7, 30][index]
        );
    }
}

async fn report(app: &Router, cookie: &str, days: i32) -> String {
    let response = call(app, &format!("/admin/audience/returns?days={days}"), cookie).await;
    private(&response, StatusCode::OK);
    body(response).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn returns_http_exact_maturity_unknown_zero_history_and_live_admin() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("AudienceReturnsTestPassword2026").unwrap();
    let mut fixtures = Fixtures {
        url: url.clone(),
        users: Vec::new(),
        coverage: Vec::new(),
        book: None,
        saved_daily: None,
    };
    let mut cookies = Vec::new();
    for kind in ["admin", "reader", "moderator"] {
        let token = generate_session_token();
        fixtures.users.push(
            accounts
                .register_with_session(
                    &format!("flow-returns-{kind}-{stamp}@example.invalid"),
                    "Returns HTTP fixture",
                    &password,
                    &session_token_digest(&token),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={token}"));
    }
    let admin = fixtures.users[0];
    let moderator = fixtures.users[2];
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"));
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
            "/admin/audience/returns?days=bad&subject_id=forged",
            cookie,
        )
        .await;
        private(&denied, status);
        assert!(!body(denied).await.contains("data-metric="));
    }
    for query in [
        "days=0",
        "days=2",
        "days=-1",
        "days=365",
        "days=2147483648",
        "days=7&days=30",
        "days=bad",
        "days=7&user_id=1",
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

    // Two closed, non-overlapping collection epochs leave a real gap. Only
    // the exact target day matters: a covered later D7 remains measurable.
    for (start, end) in [(200, "(date_trunc('day',now() AT TIME ZONE 'UTC')-interval '10 days'+interval '12 hours') AT TIME ZONE 'UTC'"),
        (8, "now()")] {
        fixtures.coverage.push(pg(&url, &format!(
            "INSERT INTO reader_return_coverage(started_at,last_heartbeat_at,ended_at)
             VALUES(({TODAY}-{start})::timestamp AT TIME ZONE 'UTC',{end},{end}) RETURNING id"
        )).parse().unwrap());
    }
    let before = [1, 7, 30, 90];
    let mut baselines = Vec::new();
    for days in before {
        let html = report(&app, &cookies[0], days).await;
        baselines.push([counts(&html, "current"), counts(&html, "previous")]);
    }

    // A matching, genuine-shaped old C fact is deliberately not new daily
    // history. It cannot manufacture a mature non-returning cohort.
    let book = pg(&url, &format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level)
        VALUES('ci:flow:returns:http:{stamp}','ru','RETURN-PRIVATE-TITLE-{stamp}','public','basic') RETURNING id")).parse::<i64>().unwrap();
    fixtures.book = Some(book);
    let reader = fixtures.users[1];
    pg(&url, &format!(
        "WITH chapter AS(INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
           VALUES({book},1,'Old reading','public','free',0) RETURNING id),
         version AS(INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
           SELECT id,1,'RETURN-PRIVATE-TEXT-{stamp}' FROM chapter RETURNING chapter_id,id),
         published AS(INSERT INTO chapter_publications(chapter_id,version_id) SELECT chapter_id,id FROM version),
         fact AS(INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,started_at,source_event_id)
           SELECT {reader},{book},chapter_id,id,now()-interval '60 days',gen_random_uuid() FROM version RETURNING *)
         INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         SELECT source_event_id,'ChapterStartedV1',1,'chapter',chapter_id,source_event_id::text,
           jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,
           'version_id',first_version_id,'authority','C-verified-free-start'),ARRAY['audience-returns-fixture-only'] FROM fact"
    ));
    let old = report(&app, &cookies[0], 90).await;
    assert_eq!(counts(&old, "current"), baselines[3][0]);
    assert_eq!(counts(&old, "previous"), baselines[3][1]);

    // Today's target cannot become mature, even with a positive return flag.
    // With collection stopped its incomplete coverage is already unknown;
    // future target days still await an observation that can yet be complete.
    for (index, (age, flags, epoch)) in [
        (0, [false, false, false], 1),
        (1, [true, false, false], 1),
        (2, [true, false, false], 1),
        (2, [false, false, false], 1),
        (7, [false, true, false], 1),
        (8, [false, true, false], 1),
        (30, [false, false, true], 0),
        (10, [true, true, false], 0),
    ]
    .into_iter()
    .enumerate()
    {
        let user = pg(&url, &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('flow-returns-cohort-{stamp}-{index}@example.invalid','Private cohort','$argon2id$fixture') RETURNING id"
        )).parse::<i64>().unwrap();
        fixtures.users.push(user);
        let epoch = fixtures.coverage[epoch];
        pg(&url, &format!(
            "WITH consent AS(INSERT INTO rec_subjects(user_id,enabled,enabled_at)
             VALUES({user},true,({TODAY}-{age})::timestamp AT TIME ZONE 'UTC') RETURNING subject_id)
             INSERT INTO reader_return_subjects(subject_id,cohort_day,coverage_epoch_id,returned_d1,returned_d7,returned_d30)
             SELECT subject_id,{TODAY}-{age},{epoch},{},{},{} FROM consent", flags[0], flags[1], flags[2]
        ));
    }
    fixtures.saved_daily = Some(pg(
        &url,
        &format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(d)),'[]'::jsonb) FROM reader_return_daily d
         WHERE cohort_day IN(SELECT {TODAY}-age FROM unnest(ARRAY[{AGES}]) age)"
        ),
    ));
    for (age, flags) in [
        (45, [1, 0, 1]),
        (89, [0, 1, 0]),
        (90, [1, 1, 1]),
        (179, [0, 0, 0]),
        (180, [1, 1, 1]),
    ] {
        for (day, returned) in [1, 7, 30].into_iter().zip(flags) {
            pg(&url, &format!(
                "INSERT INTO reader_return_daily AS d(cohort_day,day,enrolled,eligible,returned,unknown)
                 VALUES({TODAY}-{age},{day},1,1,{returned},0) ON CONFLICT(cohort_day,day) DO UPDATE SET
                 enrolled=d.enrolled+1,eligible=d.eligible+1,returned=d.returned+EXCLUDED.returned"
            ));
        }
    }

    let pending = Counts {
        enrolled: 1,
        days: [[0, 0, 1, 0]; 3],
    };
    let expectations = [
        (
            1,
            [
                pending,
                Counts {
                    enrolled: 1,
                    days: [[0, 0, 0, 1], [0, 0, 1, 0], [0, 0, 1, 0]],
                },
            ],
        ),
        (
            7,
            [
                Counts {
                    enrolled: 4,
                    days: [[2, 1, 1, 1], [0, 0, 4, 0], [0, 0, 4, 0]],
                },
                Counts {
                    enrolled: 3,
                    days: [[2, 0, 0, 1], [2, 2, 0, 1], [0, 0, 3, 0]],
                },
            ],
        ),
        (
            30,
            [
                Counts {
                    enrolled: 7,
                    days: [[4, 1, 1, 2], [2, 2, 4, 1], [0, 0, 7, 0]],
                },
                Counts {
                    enrolled: 2,
                    days: [[2, 1, 0, 0], [2, 0, 0, 0], [1, 1, 0, 1]],
                },
            ],
        ),
        (
            90,
            [
                Counts {
                    enrolled: 10,
                    days: [[7, 2, 1, 2], [5, 3, 4, 1], [2, 1, 7, 1]],
                },
                Counts {
                    enrolled: 2,
                    days: [[2, 1, 0, 0]; 3],
                },
            ],
        ),
    ];
    let subjects = pg(
        &url,
        &format!(
            "SELECT string_agg(subject_id::text,',') FROM rec_subjects WHERE user_id IN({})",
            fixtures
                .users
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    for (index, (days, expected)) in expectations.into_iter().enumerate() {
        let html = report(&app, &cookies[0], days).await;
        for (side, key) in ["current", "previous"].into_iter().enumerate() {
            delta(baselines[index][side], counts(&html, key), expected[side]);
        }
        for forbidden in [
            format!("RETURN-PRIVATE-TITLE-{stamp}"),
            format!("RETURN-PRIVATE-TEXT-{stamp}"),
            format!("flow-returns-admin-{stamp}@example.invalid"),
            cookies[0].trim_start_matches("tor-dev-session=").to_owned(),
        ] {
            assert!(
                !html.contains(&forbidden),
                "no personal/source data: {forbidden}"
            );
        }
        for subject in subjects.split(',') {
            assert!(
                !html.contains(subject),
                "consent epochs are never disclosed"
            );
        }
        assert!(html.contains("audience-returns-partial"));
        assert!(html.contains("Недостаточно завершённых сопоставимых наблюдений"));
        let repeat = report(&app, &cookies[0], days).await;
        assert_eq!(counts(&repeat, "current"), counts(&html, "current"));
        assert_eq!(counts(&repeat, "previous"), counts(&html, "previous"));
    }
    let default = call(&app, "/admin/audience/returns", &cookies[0]).await;
    private(&default, StatusCode::OK);
    let default = body(default).await;
    let ninety = report(&app, &cookies[0], 90).await;
    assert_eq!(counts(&default, "current"), counts(&ninety, "current"));
    assert_eq!(counts(&default, "previous"), counts(&ninety, "previous"));
    let overview = body(call(&app, "/admin/audience?days=30", &cookies[0]).await).await;
    assert!(overview.contains("data-testid=\"audience-returns-link\""));
    assert!(overview.contains("href=\"/admin/audience/returns?days=30\""));

    let lock = LockedReport::acquire(&url);
    let started = Instant::now();
    let unavailable = call(&app, "/admin/audience/returns?days=90", &cookies[0]).await;
    private(&unavailable, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "bounded SQL/HTTP failure"
    );
    let unavailable = body(unavailable).await;
    assert!(unavailable.contains("audience-returns-unavailable"));
    assert!(
        !unavailable.contains("data-metric="),
        "an outage is never a zero report"
    );
    drop(lock);
    assert_eq!(
        counts(&report(&app, &cookies[0], 90).await, "current"),
        counts(&ninety, "current")
    );

    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let revoked = call(&app, "/admin/audience/returns?days=90", &cookies[0]).await;
    private(&revoked, StatusCode::FORBIDDEN);
    assert!(!body(revoked).await.contains("data-metric="));
}
