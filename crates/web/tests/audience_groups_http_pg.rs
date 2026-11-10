//! FLOW-05: private group comparison and exact detail round-trips.
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
        writeln!(input, "BEGIN; LOCK TABLE organic_discovery_quality_daily IN ACCESS EXCLUSIVE MODE; SELECT 'groups-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "groups-locked");
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

fn summary<'a>(html: &'a str, surface: &str, period: &str) -> &'a str {
    let marker = format!("data-testid=\"audience-{surface}-{period}\"");
    html.split(marker.as_str())
        .nth(1)
        .expect("whole-report period")
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn group<'a>(html: &'a str, label: &str) -> &'a str {
    let marker = format!("data-label=\"{}\"", escaped(label));
    html.split("data-testid=\"audience-group-row\"")
        .skip(1)
        .find(|row| row.split('>').next().unwrap().contains(&marker))
        .expect("rendered named/missing group")
}

fn period<'a>(html: &'a str, period: &str) -> &'a str {
    let marker = format!("data-period=\"{period}\"");
    html.split(marker.as_str()).nth(1).expect("group period")
}

fn link(html: &str, name: &str) -> String {
    let marker = format!("data-testid=\"{name}\"");
    html.split('<')
        .find(|tag| tag.contains(&marker))
        .expect("navigation link")
        .split("href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .replace("&amp;", "&")
}

fn same_metrics(left: &str, right: &str) {
    for key in ["issued", "impressions", "clicks", "book-opens", "starts"] {
        assert_eq!(metric(left, key), metric(right, key), "{key}");
    }
}

fn same_period(left: &str, right: &str) {
    same_metrics(left, right);
    for key in ["no-observations", "observed-without-starts", "with-starts"] {
        let marker = format!("data-state=\"{key}\"");
        let value = |html: &str| {
            html.split(marker.as_str())
                .nth(1)
                .unwrap()
                .split('>')
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap()
                .trim()
                .to_owned()
        };
        assert_eq!(value(left), value(right), "state {key}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn groups_http_exact_details_complete_periods_and_live_admin() {
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
    let password = hash_password("AudienceGroupsTestPassword2026").unwrap();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for kind in ["admin", "reader", "moderator"] {
        let token = generate_session_token();
        fixtures.users.push(
            accounts
                .register_with_session(
                    &format!("flow-groups-{kind}-{stamp}@example.invalid"),
                    "Groups report fixture",
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
    let prefix = format!("!g{stamp}");
    let label = |index| format!("{prefix}-{index:02}");
    let special = format!("{} &+\"", label(0));
    let tag = format!("{prefix}-тег + &\"");
    let tag_second = format!("{prefix}-другой");
    let no_genre_tag = format!("{prefix}-без-жанра");
    let secret = format!("{prefix}-тайная");
    for index in 0..26 {
        let genres = match index {
            0 => format!("ARRAY['{special}','{}','{special}']", label(1)),
            1 | 2 => format!("ARRAY['{special}']"),
            3 => format!("ARRAY['{secret}']"),
            _ => format!("ARRAY['{}']", label(index)),
        };
        let tags = if index == 0 {
            format!("ARRAY['{tag}','{tag_second}','{tag}']")
        } else {
            format!("ARRAY['{tag}']")
        };
        fixtures.books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
             VALUES('ci:flow:groups:http:{stamp}:{index}','ru','Group fixture {index}','public','basic',{genres},{tags}) RETURNING id"
        )).parse().unwrap());
    }
    // More than one detail page with a genuinely absent genre, not an empty filter.
    for index in 26..49 {
        fixtures.books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
             VALUES('ci:flow:groups:http:{stamp}:{index}','ru','No genre {index}','public','basic','{{}}',ARRAY['{no_genre_tag}']) RETURNING id"
        )).parse().unwrap());
    }
    let no_tag = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
         VALUES('ci:flow:groups:http:{stamp}:no-tag','ru','No tag','public','basic',ARRAY['{}'],'{{}}') RETURNING id", label(26)
    )).parse::<i64>().unwrap();
    fixtures.books.push(no_tag);
    let (read, observed, hidden) = (fixtures.books[0], fixtures.books[1], fixtures.books[3]);
    pg(&url, &format!(
        "UPDATE books SET admin_hold=true,tags=ARRAY['{secret}'] WHERE id={hidden};
         INSERT INTO organic_discovery_quality_daily(bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-n,{read},'catalog',2,1,1,CASE WHEN n=0 THEN 1 ELSE 0 END,0 FROM generate_series(0,179) n;
         INSERT INTO organic_discovery_quality_daily(bucket_date,book_id,source,issued,clicks,book_opens,starts,completions) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{observed},'search',3,0,0,0,0),
         ((now() AT TIME ZONE 'UTC')::date,{no_tag},'catalog',0,0,0,0,0),
         ((now() AT TIME ZONE 'UTC')::date,{hidden},'catalog',999999,999999,999999,999999,999999);
         INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
         impressions,paired_clicks,unpaired_clicks,starts,completions,book_opens) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{read},'home','fallback','popular',1,'flow-groups-http','v1',7,3,1,1,0,3),
         ((now() AT TIME ZONE 'UTC')::date-90,{read},'home','fallback','popular',1,'flow-groups-http','v1',2,1,0,0,0,NULL)"
    ));
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        "http://127.0.0.1:3098".into(),
    );
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let denied = call(&app, "/admin/audience/groups?days=invalid", cookie).await;
        private(&denied, status);
        assert!(!body(denied).await.contains("audience-group-row"));
    }
    for raw in [
        "days=2",
        "days=x",
        "days=7&days=1",
        "kind=authors",
        "kind=genres&kind=tags",
        "after=",
        "after=%00",
        "actor_id=5",
    ] {
        private(
            &call(&app, &format!("/admin/audience/groups?{raw}"), &cookies[0]).await,
            StatusCode::BAD_REQUEST,
        );
    }
    for raw in [
        "without_genre=1",
        "without_genre=TRUE",
        "without_tag=x",
        "without_genre=true&genre=value",
        "without_tag=true&tag=value",
        "without_tag=true&without_tag=false",
    ] {
        private(
            &call(&app, &format!("/admin/audience/books?{raw}"), &cookies[0]).await,
            StatusCode::BAD_REQUEST,
        );
    }
    let path = format!(
        "/admin/audience/groups?kind=genres&days=90&after={}",
        encoded(&prefix)
    );
    let response = call(&app, &path, &cookies[0]).await;
    private(&response, StatusCode::OK);
    let html = body(response).await;
    assert_eq!(
        html.matches("data-testid=\"audience-group-row\"").count(),
        20
    );
    assert!(html.contains(&escaped(&special)) && !html.contains(&secret));
    for forbidden in [tokens[0].as_str(), "flow-groups-admin-", "999999"] {
        assert!(!html.contains(forbidden));
    }
    metrics(
        period(group(&html, &special), "current"),
        ["183", "7", "94", "93", "2"],
    );
    metrics(
        period(group(&html, &special), "previous"),
        ["180", "2", "91", "Нет данных", "0"],
    );
    metrics(
        period(group(&html, &label(1)), "current"),
        ["180", "7", "94", "93", "2"],
    );
    metrics(
        period(group(&html, &label(4)), "current"),
        ["Нет данных"; 5],
    );
    let detail = link(group(&html, &special), "audience-group-books");
    assert!(detail.contains("days=90") && detail.contains(&format!("genre={}", encoded(&special))));
    let detail_html = body(call(&app, &detail, &cookies[0]).await).await;
    for p in ["current", "previous"] {
        same_period(
            period(group(&html, &special), p),
            summary(&detail_html, "books", p),
        );
    }
    assert_eq!(
        detail_html
            .matches("data-testid=\"audience-book-row\"")
            .count(),
        3
    );
    let next = link(&html, "audience-groups-next");
    assert!(next.contains("kind=genres") && next.contains("days=90") && next.contains("after="));
    let second = call(&app, &next, &cookies[0]).await;
    private(&second, StatusCode::OK);
    let second = body(second).await;
    assert!(!second.contains(&format!("data-label=\"{}\"", escaped(&special))));
    assert!(second.contains(&format!("data-label=\"{}\"", label(25))));
    metrics(
        period(group(&second, &label(26)), "current"),
        ["0", "Нет данных", "0", "0", "0"],
    );
    for p in ["current", "previous"] {
        same_metrics(summary(&html, "groups", p), summary(&second, "groups", p));
    }
    let tags_path = path.replace("kind=genres", "kind=tags");
    let tags = body(call(&app, &tags_path, &cookies[0]).await).await;
    for p in ["current", "previous"] {
        same_metrics(
            period(group(&html, &special), p),
            period(group(&tags, &tag), p),
        );
    }
    let tag_detail = link(group(&tags, &tag), "audience-group-books");
    assert!(
        tag_detail.contains(&format!("tag={}", encoded(&tag))) && tag_detail.contains("days=90")
    );
    let tag_html = body(call(&app, &tag_detail, &cookies[0]).await).await;
    for p in ["current", "previous"] {
        same_period(
            period(group(&tags, &tag), p),
            summary(&tag_html, "books", p),
        );
    }

    for kind in ["genres", "tags"] {
        let overview = body(
            call(
                &app,
                &format!("/admin/audience/groups?kind={kind}&days=90"),
                &cookies[0],
            )
            .await,
        )
        .await;
        let missing = group(&overview, "");
        assert!(missing
            .split('>')
            .next()
            .unwrap()
            .contains("data-missing=\"true\""));
        let detail = link(missing, "audience-group-books");
        let field = if kind == "genres" {
            "without_genre=true"
        } else {
            "without_tag=true"
        };
        assert!(detail.contains(field) && detail.contains("days=90"));
        let detail = body(call(&app, &detail, &cookies[0]).await).await;
        for p in ["current", "previous"] {
            same_period(period(missing, p), summary(&detail, "books", p));
        }
        if kind == "genres" {
            let next = link(&detail, "audience-books-next");
            assert!(next.contains(field) && next.contains("days=90") && next.contains("after_id="));
            let later = body(call(&app, &next, &cookies[0]).await).await;
            for p in ["current", "previous"] {
                same_metrics(summary(&detail, "books", p), summary(&later, "books", p));
            }
        }
    }
    for (days, issued, clicks, opens) in [
        (1, "5", "5", "4"),
        (7, "17", "11", "10"),
        (30, "63", "34", "33"),
    ] {
        let other = body(
            call(
                &app,
                &path.replace("days=90", &format!("days={days}")),
                &cookies[0],
            )
            .await,
        )
        .await;
        metrics(
            period(group(&other, &special), "current"),
            [issued, "7", clicks, opens, "2"],
        );
    }
    // Existing facts move with current metadata, then disappear immediately on hold.
    pg(
        &url,
        &format!(
            "UPDATE books SET genres=ARRAY['{}'] WHERE id={read}",
            label(4)
        ),
    );
    let changed = body(call(&app, &path, &cookies[0]).await).await;
    metrics(
        period(group(&changed, &special), "current"),
        ["3", "Нет данных", "0", "0", "0"],
    );
    metrics(
        period(group(&changed, &label(4)), "current"),
        ["180", "7", "94", "93", "2"],
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={read}"),
    );
    let hidden = body(call(&app, &path, &cookies[0]).await).await;
    metrics(
        period(group(&hidden, &label(4)), "current"),
        ["Нет данных"; 5],
    );
    let lock = LockedReport::acquire(&url);
    let unavailable = call(&app, &path, &cookies[0]).await;
    private(&unavailable, StatusCode::SERVICE_UNAVAILABLE);
    let unavailable = body(unavailable).await;
    assert!(
        unavailable.contains("audience-groups-unavailable")
            && !unavailable.contains("audience-group-row")
    );
    pg(&url,&format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    private(&call(&app, &path, &cookies[0]).await, StatusCode::FORBIDDEN);
    drop(lock);
    pg(&url,&format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
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
