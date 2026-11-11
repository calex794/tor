//! Real admin session, provider snapshots, refresh commands and isolated refusal.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
struct LockedSeo {
    child: Child,
    input: ChildStdin,
}
impl LockedSeo {
    fn acquire(url: &str) -> Self {
        let parsed = reqwest::Url::parse(url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), Some(5432));
        assert_eq!(parsed.path(), "/tor_ci");
        assert!(parsed.query().is_none() && parsed.fragment().is_none());
        let mut child=Command::new("psql").env("PGOPTIONS","-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000")
            .args(["-X","-A","-t","-q","-v","ON_ERROR_STOP=1","--dbname",url])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let mut s = Self { child, input };
        writeln!(
            s.input,
            "BEGIN; LOCK TABLE flow_seo_daily IN ACCESS EXCLUSIVE MODE; SELECT 'seo-locked';"
        )
        .unwrap();
        s.input.flush().unwrap();
        let mut line = String::new();
        BufReader::new(s.child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "seo-locked");
        s
    }
}
impl Drop for LockedSeo {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}
async fn refresh(app: &Router, cookie: &str, origin: &str, body: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/audience/seo/refresh")
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}
fn value(html: &str, period: &str, metric: &str) -> String {
    html.split(&format!("data-testid=\"seo-{period}\""))
        .nth(1)
        .expect("SEO period")
        .split("</section>")
        .next()
        .unwrap()
        .split(&format!("data-metric=\"{metric}\">"))
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_owned()
}
pub(super) async fn check(app: &Router, url: &str, cookies: &[String], book: i64, stamp: u128) {
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        for route in [
            "/admin/audience/seo?days=bad",
            "/admin/audience/seo/refresh",
        ] {
            let response = if route.ends_with("refresh") {
                refresh(app, cookie, ORIGIN, "broken").await
            } else {
                call(app, "GET", route, cookie, "", "").await
            };
            assert_eq!(response.status(), status);
            private(&response);
            assert!(!page(response).await.contains("data-metric="));
        }
    }
    assert_eq!(
        pg(url, "SELECT count(*) FROM flow_seo_sources WHERE enabled"),
        "0",
        "preceding SEO fixtures must clean up their own sources"
    );
    let empty = page(call(app, "GET", "/admin/audience/seo", &cookies[0], "", "").await).await;
    assert!(empty.contains("seo-unconfigured"));
    assert_ne!(value(&empty, "current", "clicks"), "0");
    let source:i64=pg(url,&format!("INSERT INTO flow_seo_sources(provider,property,origin) VALUES('google','https://seo-http-{stamp}.test/','https://seo-http-{stamp}.test') RETURNING id")).parse().unwrap();
    pg(url,&format!("INSERT INTO flow_seo_daily(source_id,day,status,total_clicks,total_impressions,total_position,provider_rows,omitted_rows,limit_reached) VALUES({source},(now() AT TIME ZONE 'UTC')::date-3,'available',20,100,3,1,0,false); INSERT INTO flow_seo_pages(source_id,day,path,clicks,impressions,position) VALUES({source},(now() AT TIME ZONE 'UTC')::date-3,'/books/{book}',2,10,4)"));
    let raw_before=pg(url,"SELECT (SELECT count(*) FROM rec_subjects)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM outbox_events)");
    for days in [1, 7, 30, 90] {
        let response = call(
            app,
            "GET",
            &format!("/admin/audience/seo?days={days}&provider=google"),
            &cookies[0],
            "",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let html = page(response).await;
        assert_eq!(value(&html, "current", "clicks"), "20");
        assert!(html.contains(&format!("href=\"/books/{book}\"")));
        assert!(html.contains("seo-comparison-unavailable"));
        assert!(!html.contains("Audience text"));
        assert!(!html.contains("audience-reader-"));
    }
    for query in [
        "days=2",
        "provider=other",
        "after=/admin",
        "after=/books/01",
        "days=7&days=7",
        "unknown=true",
    ] {
        let r = call(
            app,
            "GET",
            &format!("/admin/audience/seo?{query}"),
            &cookies[0],
            "",
            "",
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        private(&r);
    }
    let yandex = page(
        call(
            app,
            "GET",
            "/admin/audience/seo?provider=yandex",
            &cookies[0],
            "",
            "",
        )
        .await,
    )
    .await;
    assert!(yandex.contains("seo-unconfigured"));
    assert_ne!(value(&yandex, "current", "clicks"), "20");
    // The form token is a standard session-independent double-submit cookie.
    let csrf = generate_session_token();
    let cookie = format!("{}; tor-dev-csrf={csrf}", cookies[0]);
    let good = format!("provider=google&csrf_token={csrf}");
    for (origin, body) in [
        (ORIGIN, "provider=google&csrf_token=bad"),
        ("https://foreign.invalid", good.as_str()),
    ] {
        let r = refresh(app, &cookie, origin, body).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        private(&r);
    }
    for _ in 0..2 {
        let r = refresh(app, &cookie, ORIGIN, &good).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        private(&r);
    }
    assert_eq!(
        pg(
            url,
            &format!("SELECT count(*) FROM flow_seo_jobs WHERE source_id={source}")
        ),
        "7"
    );
    let r = refresh(
        app,
        &cookie,
        ORIGIN,
        &format!("provider=other&csrf_token={csrf}"),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    private(&r);
    let r = refresh(
        app,
        &cookie,
        ORIGIN,
        &format!("provider=google&csrf_token={}", "x".repeat(5000)),
    )
    .await;
    assert!(r.status().is_client_error());
    private(&r);
    let owner = std::env::var("MIGRATOR_URL").unwrap();
    let lock = LockedSeo::acquire(&owner);
    let t = std::time::Instant::now();
    let r = call(app, "GET", "/admin/audience/seo", &cookies[0], "", "").await;
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    private(&r);
    assert!(t.elapsed() < std::time::Duration::from_secs(3));
    let html = page(r).await;
    assert!(html.contains("seo-unavailable"));
    assert!(!html.contains("data-metric="));
    drop(lock);
    let recovered = page(call(app, "GET", "/admin/audience/seo", &cookies[0], "", "").await).await;
    assert_eq!(value(&recovered, "current", "clicks"), "20");
    pg(
        url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let hidden = page(call(app, "GET", "/admin/audience/seo", &cookies[0], "", "").await).await;
    assert!(!hidden.contains(&format!("href=\"/books/{book}\"")));
    assert_eq!(value(&hidden, "current", "clicks"), "20");
    pg(
        url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    assert_eq!(pg(url,"SELECT (SELECT count(*) FROM rec_subjects)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM outbox_events)"),raw_before);
    pg(
        url,
        &format!("DELETE FROM flow_seo_sources WHERE id={source}"),
    );
}
