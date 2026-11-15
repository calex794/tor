//! An unavailable aggregate must never render fabricated zero counts.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
};

struct AggregateLock(Child);

impl AggregateLock {
    fn acquire(url: &str) -> Self {
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        let mut child = Command::new("psql")
            .args([
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "--dbname",
                url,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(
            b"BEGIN; SET LOCAL lock_timeout='2s'; LOCK TABLE flow_book_cohorts IN ACCESS EXCLUSIVE MODE; SELECT 'FLOW_FUNNEL_LOCK';\n").unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "FLOW_FUNNEL_LOCK");
        Self(child)
    }
}

impl Drop for AggregateLock {
    fn drop(&mut self) {
        if let Some(mut input) = self.0.stdin.take() {
            let _ = input.write_all(b"ROLLBACK;\n\\q\n");
        }
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn unavailable_funnel_is_private_503_and_does_not_stop_allowed_html() {
    let h = setup().await;
    let (browser, admin) = registration(
        &h.app,
        &h.url,
        &format!("funnel-outage-{}@tor-ci.example", h.stamp),
    )
    .await;
    pg(
        &h.url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let lock = AggregateLock::acquire(&h.url);
    let started = std::time::Instant::now();
    assert_eq!(browser.get(&h.app, "/").await.status(), StatusCode::OK);
    assert!(started.elapsed() < Duration::from_millis(950));
    let response = browser.get(&h.app, "/admin/audience/funnel").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    private(&response);
    let html = text(response).await;
    assert!(html.contains("data-testid=\"flow-funnel-unavailable\""));
    assert!(!html.contains("data-testid=\"flow-funnel-current\""));
    assert!(!html.contains("data-metric=\"cohorts\""));
    drop(lock);
    assert_eq!(
        browser.get(&h.app, "/admin/audience/funnel").await.status(),
        StatusCode::OK
    );
    pg(
        &h.url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={admin} AND role='administrator'"
        ),
    );
}
