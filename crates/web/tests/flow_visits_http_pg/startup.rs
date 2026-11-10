//! Actual AUTH=0 binary on a real installed190 -> 192 database, never a helper drain.
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[path = "startup_preferences.rs"]
mod preferences;
#[path = "startup_retirement.rs"]
mod retirement;

fn pg(url: &str, sql: &str) -> String {
    let output = Command::new("psql")
        .args([
            "--no-psqlrc",
            "--set=ON_ERROR_STOP=1",
            "--tuples-only",
            "--no-align",
            "--quiet",
        ])
        .arg("--dbname")
        .arg(url)
        .arg("--command")
        .arg(sql)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct BusyRetirement {
    child: Child,
    stdin: Option<ChildStdin>,
}
impl BusyRetirement {
    fn hold(url: &str) -> Self {
        let mut child = Command::new("psql")
            .args([
                "--no-psqlrc",
                "--set=ON_ERROR_STOP=1",
                "--tuples-only",
                "--no-align",
                "--quiet",
            ])
            .arg("--dbname")
            .arg(url)
            .env(
                "PGOPTIONS",
                "-c statement_timeout=5000 -c idle_in_transaction_session_timeout=30000",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "BEGIN; SELECT pg_advisory_xact_lock(20261116,8903); SELECT 'HELD';"
        )
        .unwrap();
        stdin.flush().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "retirement gate holder exited"
            );
            if line.trim() == "HELD" {
                break;
            }
        }
        Self {
            child,
            stdin: Some(stdin),
        }
    }
}
impl Drop for BusyRetirement {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "ROLLBACK;\n\\q");
            let _ = stdin.flush();
        }
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires installed192 disposable tor_ci_collection_retirement_startup"]
async fn public_postgres_startup_retires_legacy_flags_and_preserves_cleanup() {
    let url = std::env::var("COLLECTION_RETIREMENT_STARTUP_APP_DATABASE_URL").unwrap();
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(5432));
    assert_eq!(parsed.username(), "tor_app");
    assert_eq!(parsed.path(), "/tor_ci_collection_retirement_startup");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    assert_eq!(
        pg(&url, "SELECT current_database()||'|'||current_user"),
        "tor_ci_collection_retirement_startup|tor_app"
    );
    assert_eq!(
        pg(
            &url,
            "SELECT concat_ws('|',count(*),max(version),bool_and(success)) FROM _sqlx_migrations"
        ),
        "192|202611168903|t"
    );
    let legacy = retirement::Probe::load(&url);
    let canonical = legacy.canonical(&url);
    let choices = legacy.preferences(&url);
    let before_raw = retirement::raw(&url);
    assert!(
        !retirement::drained(&url),
        "bootstrap must leave real pre-barrier raw data"
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = pg(&url, &format!(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('retirement-startup-{stamp}@ci.invalid','Current preference reader','$argon2id$test')
         RETURNING id"
    )).parse().unwrap();
    let preference_probe = preferences::seed(&url, actor, legacy.book).await;
    let busy = BusyRetirement::hold(&url);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_tor-web"))
            .env_clear()
            .env("DATABASE_URL", &url)
            .env("TOR_LISTEN_ADDR", address.to_string())
            .env("TOR_DEMO_MODE", "0")
            .env("TOR_AUTH_ENABLED", "0")
            .env("TOR_RECOMMENDATIONS_ENABLED", "0")
            .env("TOR_FLOW_VISITS_ENABLED", "1")
            .env("TOR_REC_ATTRIBUTION_ENABLED", "1")
            .env("TOR_ORGANIC_ATTRIBUTION_ENABLED", "1")
            .env("TOR_READING_RETURNS_ENABLED", "1")
            .env("TOR_BOOK_READING_RETURNS_ENABLED", "1")
            .env("TOR_FLOW_PUBLIC_OPT_IN_ENABLED", "1")
            .env("TOR_RIL_SINGLE_INSTANCE", "1")
            .env("TOR_RIL_HMAC_KEY", "11".repeat(32))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("production tor-web"),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(750))
        .build()
        .unwrap();
    let origin = format!("http://{address}");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(server.0.try_wait().unwrap().is_none(), "server exited");
        if client
            .get(format!("{origin}/readyz"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
            && preferences::expired(&url, actor, &preference_probe)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "HTTP or independent P1 cleanup blocked behind retirement"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        retirement::raw(&url),
        before_raw,
        "a busy erasure never reports a completed drain"
    );
    assert_eq!(legacy.canonical(&url), canonical);
    assert_eq!(legacy.preferences(&url), choices);
    preferences::assert_preserved(&url, actor, &preference_probe);
    drop(busy);
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited during bounded cleanup"
        );
        if retirement::drained(&url) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real supervisor failed to resume the bounded erasure"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    retirement::assert_aggregates(&url, &legacy);
    assert_eq!(legacy.canonical(&url), canonical);
    assert_eq!(legacy.preferences(&url), choices);
    for _ in 0..3 {
        assert!(client
            .get(format!("{origin}/"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    let late = legacy.accept_late_c(&url).await;
    // A second real preference expiry is a witness that another supervisor
    // iteration ran after this canonical command. The test invokes no worker.
    preferences::arm_next_pass(&url, actor, legacy.book, &preference_probe).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !preferences::next_pass_done(&url, actor, legacy.book, &preference_probe) {
        assert!(server.0.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "next always-on retention pass did not run"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    legacy.assert_late_c(&url, &late);
    assert_eq!(legacy.canonical(&url), canonical);
    assert!(retirement::drained(&url));
    retirement::assert_aggregates(&url, &legacy);
    preferences::assert_live_unchanged(&url, actor, &preference_probe);
    assert_eq!(legacy.preferences(&url), choices);
    drop(server);
    preferences::cleanup(&url, &preference_probe);
    pg(&url, &format!("DELETE FROM users WHERE id={actor}"));
}
