//! Database outage is optional telemetry loss, never a reader access dependency.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
    time::Instant,
};

struct VisitLock(Child);

impl VisitLock {
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
            b"BEGIN; SET LOCAL lock_timeout='2s'; LOCK TABLE flow_visits IN ACCESS EXCLUSIVE MODE; SELECT 'FLOW_LOCK_READY';\n",
        ).unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "FLOW_LOCK_READY");
        Self(child)
    }
}

impl Drop for VisitLock {
    fn drop(&mut self) {
        if let Some(mut input) = self.0.stdin.take() {
            let _ = input.write_all(b"ROLLBACK;\n\\q\n");
        }
        let _ = self.0.wait();
    }
}

fn dropped(html: &str) -> u64 {
    html.split("Переполнения очереди или записи без подтверждения</dt><dd>")
        .nth(1)
        .expect("current process loss counter")
        .split('<')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn locked_visit_storage_does_not_block_reading_and_report_fails_explicitly() {
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool),
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let (admin_browser, admin) = registration(
        &app,
        &url,
        &format!("flow-outage-admin-{stamp}@tor-ci.example"),
    )
    .await;
    let (reader_browser, reader) = registration(
        &app,
        &url,
        &format!("flow-outage-reader-{stamp}@tor-ci.example"),
    )
    .await;
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    set_choice(&reader_browser, &app, REC_FORM, true).await;
    set_choice(&reader_browser, &app, FLOW_FORM, true).await;
    let initial = admin_browser.get(&app, "/admin/audience/visits").await;
    assert_eq!(initial.status(), StatusCode::OK);
    let dropped_before = dropped(&text(initial).await);
    let lock = VisitLock::acquire(&url);
    let started = Instant::now();
    let allowed = reader_browser.get(&app, "/").await;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert!(
        started.elapsed() < Duration::from_millis(950),
        "telemetry write delayed allowed HTML"
    );
    let mut saw_drop = false;
    // Each bounded report waits for the real lock and must return an honest
    // 503. The queue exhausts its three retries without stopping the page.
    for _ in 0..6 {
        let response = admin_browser.get(&app, "/admin/audience/visits").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        private(&response);
        let html = text(response).await;
        assert!(html.contains("data-testid=\"flow-visits-unavailable\""));
        assert!(!html.contains("data-testid=\"flow-visits-current\""));
        if dropped(&html) > dropped_before {
            saw_drop = true;
            break;
        }
    }
    assert!(
        saw_drop,
        "failed optional write must be visible as a dropped observation"
    );
    drop(lock);
    assert_eq!(
        own_pages(&url, reader),
        0,
        "failed transaction did not persist a partial page"
    );
    assert_eq!(reader_browser.get(&app, "/").await.status(), StatusCode::OK);
    wait_pages(&url, reader, 1).await;
    assert_eq!(
        admin_browser
            .get(&app, "/admin/audience/visits")
            .await
            .status(),
        StatusCode::OK
    );
    set_choice(&reader_browser, &app, FLOW_FORM, false).await;
    pg(&url, &format!("DELETE FROM account_roles WHERE user_id={admin}; DELETE FROM users WHERE id IN({admin},{reader})"));
}
