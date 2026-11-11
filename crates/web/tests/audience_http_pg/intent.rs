//! Real guarded product writes followed by independent read-only admin snapshots.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
};

// Keep SQL fixture machinery outside tor-web's dependencies. A real psql
// transaction holds the relation lock until Drop, including assertion failures.
struct LockedFollows {
    child: Child,
    input: ChildStdin,
}
impl LockedFollows {
    fn acquire(url: &str) -> Self {
        let target = reqwest::Url::parse(url).expect("fixture database URL");
        assert!(matches!(target.scheme(), "postgres" | "postgresql"));
        assert_eq!(target.host_str(), Some("127.0.0.1"));
        assert_eq!(target.port(), Some(5432));
        assert_eq!(target.path(), "/tor_ci");
        assert!(target.query().is_none() && target.fragment().is_none());
        let mut child = Command::new("psql")
            .env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000")
            .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("disposable PostgreSQL client");
        let input = child.stdin.take().unwrap();
        let mut guard = Self { child, input };
        writeln!(guard.input, "BEGIN; LOCK TABLE reader_book_follows IN ACCESS EXCLUSIVE MODE; SELECT 'follows-locked';").unwrap();
        guard.input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(guard.child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "follows-locked");
        guard
    }
}
impl Drop for LockedFollows {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

async fn post(app: &Router, path: &str, cookie: &str, csrf: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn value(html: &str, cohort: &str, name: &str) -> i64 {
    let marker = format!("data-testid=\"audience-intents-{cohort}\"");
    let section = html
        .split(&marker)
        .nth(1)
        .expect("intent cohort")
        .split("</div>")
        .next()
        .unwrap();
    let metric = format!("data-metric=\"{name}\">");
    section
        .split(&metric)
        .nth(1)
        .expect("measured counter")
        .split('<')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

pub(super) async fn choices(app: &Router, cookies: (&str, &str, &str), book: i64) {
    let (admin, reader, csrf) = cookies;
    let before = page(call(app, "GET", "/admin/audience?days=7", admin, "", "").await).await;
    for route in [
        format!("/me/library/{book}/save"),
        format!("/me/books/{book}/follow"),
    ] {
        assert_eq!(
            post(app, &route, reader, "wrong").await.status(),
            StatusCode::FORBIDDEN
        );
        for _ in 0..2 {
            let result = post(app, &route, reader, csrf).await;
            assert_eq!(result.status(), StatusCode::SEE_OTHER);
            assert_eq!(result.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
    let snapshot = call(app, "GET", "/admin/audience?days=7", admin, "", "").await;
    assert_eq!(snapshot.status(), StatusCode::OK);
    private(&snapshot);
    let snapshot = page(snapshot).await;
    for name in [
        "library-links",
        "library-accounts",
        "follow-links",
        "follow-accounts",
    ] {
        for cohort in ["total", "current"] {
            assert_eq!(
                value(&snapshot, cohort, name),
                value(&before, cohort, name) + 1
            );
        }
        assert_eq!(
            value(&snapshot, "previous", name),
            value(&before, "previous", name)
        );
    }
    // Lock just this independent source, not auth or canonical C evidence.
    let owner_url = std::env::var("MIGRATOR_URL").unwrap();
    let lock = LockedFollows::acquire(&owner_url);
    let started = std::time::Instant::now();
    let failed = call(app, "GET", "/admin/audience?days=7", admin, "", "").await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    private(&failed);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    let failed = page(failed).await;
    assert!(failed.contains("audience-intents-unavailable"));
    assert!(!failed.contains("data-testid=\"audience-intents-total\""));
    assert!(failed.contains("data-testid=\"audience-current\""));
    drop(lock);
    let recovered = call(app, "GET", "/admin/audience?days=7", admin, "", "").await;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(
        value(&page(recovered).await, "total", "library-links"),
        value(&snapshot, "total", "library-links")
    );
    for route in [
        format!("/me/library/{book}/remove"),
        format!("/me/books/{book}/unfollow"),
    ] {
        assert_eq!(
            post(app, &route, reader, csrf).await.status(),
            StatusCode::SEE_OTHER
        );
    }
    let removed = page(call(app, "GET", "/admin/audience?days=7", admin, "", "").await).await;
    for name in [
        "library-links",
        "library-accounts",
        "follow-links",
        "follow-accounts",
    ] {
        for cohort in ["total", "current"] {
            assert_eq!(value(&removed, cohort, name), value(&before, cohort, name));
        }
    }
    // Restore one real choice for the later hidden-book and privacy assertions.
    for route in [
        format!("/me/library/{book}/save"),
        format!("/me/books/{book}/follow"),
    ] {
        assert_eq!(
            post(app, &route, reader, csrf).await.status(),
            StatusCode::SEE_OTHER
        );
    }
}
