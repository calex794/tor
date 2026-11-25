//! ADAPT-02d: real Axum recommendation HTTP over loopback TCP and shared PG18.
#![forbid(unsafe_code)]
#[path = "../../../tests/support/adapt_pacer.rs"]
mod adapt_pacer;
use adapt_pacer::{classify, percentile, Pacer, Policy, Pressure, MAX_MS};
use serde_json::Value;
use std::{process::Command, sync::{Arc, Mutex}, time::Duration};
use tokio::time::{sleep_until, timeout_at, Instant};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::PgRecommendations,
    services::contextual::{Action, Control},
};

const RUN: Duration = Duration::from_secs(24);
const GRACE: Duration = Duration::from_secs(5);
const FRESH: Duration = Duration::from_secs(1);
type Samples = Mutex<Vec<(Instant, Option<u64>)>>;

fn sql(url: &str, statement: &str) -> String {
    let output = Command::new("psql")
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", statement])
        .env("PGOPTIONS", "-c statement_timeout=60000 -c lock_timeout=5000")
        .output()
        .unwrap();
    assert!(output.status.success(), "disposable fixture SQL failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap()
}

fn fresh(samples: &Samples, now: Instant) -> Vec<Option<u64>> {
    samples.lock().unwrap().iter()
        .filter(|(at, _)| now.saturating_duration_since(*at) <= FRESH)
        .map(|(_, value)| *value)
        .collect()
}

#[derive(Debug)]
enum Outcome {
    Ok(u64, u64),
    Http(u16),
    Transport,
    Timeout,
    Unstarted,
}

async fn read(client: &reqwest::Client, path: &str) -> Result<(u16, Value), reqwest::Error> {
    let response = client.get(path).send().await?;
    let status = response.status().as_u16();
    let body = if status == 200 { response.json().await? }
        else { let _ = response.bytes().await?; Value::Null };
    Ok((status, body))
}

fn check_feed(body: &Value, expected: &Value, seed: i64, sentinel: i64) {
    assert!(body["generation_id"].is_null());
    assert_eq!(body["personalized"], false);
    let items = body["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert!(item.get("ticket").is_none());
        assert_eq!(item["algorithm_version"], "catalog_new_releases_v1");
        let id = item["book_id"].as_str().unwrap().parse::<i64>().unwrap();
        assert!(id != seed && id != sentinel);
    }
    assert_eq!(body, expected, "held generation changed visible feed");
}

struct Driver {
    client: reqwest::Client,
    path: String,
    expected: Value,
    seed: i64,
    sentinel: i64,
    samples: Samples,
}

impl Driver {
    async fn offer(&self, due: Instant, end: Instant) -> Outcome {
        sleep_until(due).await;
        let begun = Instant::now();
        if begun >= end { return Outcome::Unstarted; }
        let result = timeout_at((begun + GRACE).min(end), read(&self.client, &self.path)).await;
        let now = Instant::now();
        let outcome = match result {
            Ok(Ok((200, body))) => {
                check_feed(&body, &self.expected, self.seed, self.sentinel);
                Outcome::Ok(us(now - due), us(now - begun))
            }
            Ok(Ok((status, _))) => Outcome::Http(status),
            Ok(Err(_)) => Outcome::Transport,
            Err(_) => Outcome::Timeout,
        };
        self.samples.lock().unwrap().push((now, match &outcome {
            Outcome::Ok(scheduled, _) => Some(*scheduled),
            _ => None,
        }));
        outcome
    }
}

async fn probes(catalog: &PgCatalog, start: Instant, samples: &Samples) {
    let pool = catalog.pool();
    let end = start + RUN + GRACE;
    for tick in 0..120 {
        sleep_until(start + Duration::from_millis(200) * tick).await;
        let begun = Instant::now();
        if begun >= end { break; }
        let result = timeout_at((begun + GRACE).min(end), pool.acquire()).await;
        let now = Instant::now();
        let value = match result {
            Ok(Ok(connection)) => { drop(connection); Some(us(now - begun)) }
            _ => None,
        };
        samples.lock().unwrap().push((now, value));
    }
}

#[derive(Default, Debug)]
struct Progress {
    pages: u64,
    errors: u64,
    timeouts: u64,
    idle: u64,
    max_gap_us: u64,
    decisions: Vec<(u64, Pressure, u64)>,
    phase_pages: [u64; 3],
}

async fn build(store: &PgRecommendations, start: Instant, policy: Policy,
    http: &Samples, probe: &Samples, total: i64) -> Progress {
    let end = start + RUN;
    let mut pacer = Pacer::new(policy);
    let mut progress = Progress::default();
    let mut last = start;
    sleep_until(start).await;
    while Instant::now() < end {
        let result = tokio::time::timeout(Duration::from_secs(2), store.advance_contextual_generation()).await;
        let now = Instant::now();
        let delay = match result {
            Ok(Ok(true)) => {
                progress.pages += 1;
                let elapsed = now - start;
                let phase = if elapsed < Duration::from_secs(6) { 0 }
                    else if elapsed < Duration::from_secs(14) { 1 } else { 2 };
                progress.phase_pages[phase] += 1;
                progress.max_gap_us = progress.max_gap_us.max(us(now - last));
                last = now;
                let signal = classify(fresh(http, now), fresh(probe, now));
                let delay = pacer.after_progress(signal, total.saturating_sub(progress.pages as i64), elapsed);
                progress.decisions.push((us(elapsed), signal, delay));
                delay
            }
            Ok(Ok(false)) => { progress.idle += 1; MAX_MS }
            Ok(Err(_)) => { progress.errors += 1; MAX_MS }
            Err(_) => { progress.timeouts += 1; MAX_MS }
        };
        sleep_until((now + Duration::from_millis(delay)).min(end)).await;
    }
    progress.max_gap_us = progress.max_gap_us.max(us(Instant::now() - last));
    progress
}

// Combined test process only: cumulative CPU ticks and RSS kB.
fn process_sample() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    let cpu = fields.get(11)?.parse::<u64>().ok()?.checked_add(fields.get(12)?.parse().ok()?)?;
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss = status.lines().find(|s| s.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some((cpu, rss))
}

async fn process_samples(start: Instant) -> Vec<(u64, Option<(u64, u64)>)> {
    let mut samples = Vec::new();
    for second in 0..=24 {
        sleep_until(start + Duration::from_secs(second)).await;
        samples.push((us(Instant::now() - start), process_sample()));
    }
    samples
}

async fn control(store: &PgRecommendations, action: Action, generation: Option<String>) -> String {
    let revision = store.contextual_status().await.unwrap().revision;
    store.control_contextual(&Control { action, expected_revision: revision, generation })
        .await.unwrap().generation.unwrap_or_default()
}

fn report(round: usize, policy: Policy, outcomes: &[(usize, Outcome)]) {
    for phase in 0..3 {
        let mut scheduled = Vec::new();
        let mut service = Vec::new();
        let (mut errors, mut transport, mut timeout, mut unstarted) = (0, 0, 0, 0);
        let mut statuses = std::collections::BTreeMap::new();
        for (_, outcome) in outcomes.iter().filter(|(p, _)| *p == phase) {
            match outcome {
                Outcome::Ok(a, b) => { scheduled.push(*a); service.push(*b); }
                Outcome::Http(status) => { errors += 1; *statuses.entry(*status).or_insert(0usize) += 1; }
                Outcome::Transport => transport += 1,
                Outcome::Timeout => timeout += 1,
                Outcome::Unstarted => unstarted += 1,
            }
        }
        let offered = [12, 240, 20][phase];
        assert_eq!(scheduled.len() + errors + transport + timeout + unstarted, offered);
        eprintln!("ADAPT_HTTP round={round} policy={policy:?} phase={phase} offered={offered} ok={} http_errors={errors} statuses={statuses:?} transport={transport} timeout={timeout} unstarted={unstarted} scheduled_p95_us={:?} scheduled_p99_us={:?} http_p95_us={:?} http_p99_us={:?}", scheduled.len(), percentile(scheduled.clone(),95),percentile(scheduled,99),percentile(service.clone(),95),percentile(service,99));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires GitHub CI disposable PostgreSQL 18 tor_ci and loopback TCP"]
async fn adapt_http_pacing_comparison() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("@127.0.0.1:5432/tor_ci"));
    assert_eq!(sql(&url, "SELECT current_database()||'|'||current_user"), "tor_ci|tor_app");
    assert_eq!(sql(&url, "SELECT current_setting('server_version_num')::int / 10000"), "18");
    assert_eq!(sql(&url, "SELECT count(*) FROM books WHERE source_identity LIKE 'ci:adapt-http:%'"), "0");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let store = PgRecommendations::new(pool.clone());
    assert!(store.contextual_status().await.unwrap().held);
    assert_eq!(sql(&url, "SELECT count(*) FROM rec_generations WHERE status='building'"), "0");
    let seed = sql(&url, "BEGIN; LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE;
        WITH inserted AS (
          INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags)
          SELECT (SELECT coalesce(max(id),0) FROM books)+n, 'ci:adapt-http:'||n,
                 'ru-adapt-http','ADAPT HTTP '||n,'public','basic',ARRAY['shared'],ARRAY['system']
          FROM generate_series(1,100000) n RETURNING id)
        SELECT min(id) FROM inserted; COMMIT;").parse::<i64>().unwrap();
    let sentinel = sql(&url, "INSERT INTO books(id,source_identity,target_language,title,visibility,access_level)
        SELECT coalesce(max(id),0)+1,'ci:adapt-http:sentinel','ru-adapt-http','Hidden sentinel','draft','basic' FROM books RETURNING id").parse::<i64>().unwrap();
    let total = sql(&url, &format!("SELECT count(*) FROM rec_materializable_books WHERE id>={seed}")).parse::<i64>().unwrap();
    assert_eq!(total, 100000);
    std::env::set_var("TOR_RECOMMENDATIONS_ENABLED", "1");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(), PgReader::new(pool.clone()), PgAccounts::from_pool(pool.clone()), false, origin.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy()
        .redirect(reqwest::redirect::Policy::none()).http1_only().build().unwrap();
    let path = format!("{origin}/api/v1/books/{seed}/recommendations?limit=20");
    let hidden_path = format!("{origin}/api/v1/books/{sentinel}/recommendations?limit=20");
    let (status, expected) = tokio::time::timeout(GRACE, read(&client, &path)).await.unwrap().unwrap();
    assert_eq!(status, 200);
    check_feed(&expected, &expected, seed, sentinel);
    for (round, order) in [[Policy::Baseline, Policy::ProgressBudget],
        [Policy::ProgressBudget, Policy::Baseline]].into_iter().enumerate() {
        for policy in order {
            let generation = control(&store, Action::Refresh, None).await;
            assert!(!generation.is_empty() && generation.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            sql(&url, &format!("UPDATE rec_generations SET cursor_book_id={} WHERE id='{generation}'::uuid", seed-1));
            let driver = Arc::new(Driver { client: client.clone(), path: path.clone(), expected: expected.clone(), seed, sentinel, samples: Samples::default() });
            let probe = Samples::default();
            let start = Instant::now() + Duration::from_millis(100);
            let mut tasks = Vec::new();
            for (phase, first, seconds, rate) in [(0, 0, 6, 2), (1, 6, 8, 30), (2, 14, 10, 2)] {
                for tick in 0..seconds * rate {
                    let offset = Duration::from_millis(first * 1000 + tick * 1000 / rate);
                    let driver = driver.clone();
                    tasks.push((phase, tokio::spawn(async move { driver.offer(start + offset, start + RUN + GRACE).await })));
                }
            }
            let gather = async {
                let mut outcomes = Vec::new();
                for (phase, task) in tasks { outcomes.push((phase, task.await.unwrap())); }
                outcomes
            };
            let (progress, (), outcomes, samples) = tokio::join!(
                build(&store, start, policy, &driver.samples, &probe, total),
                probes(&catalog, start, &probe), gather, process_samples(start));
            report(round + 1, policy, &outcomes);
            assert_eq!(sql(&url, &format!("SELECT status FROM rec_generations WHERE id='{generation}'::uuid")), "building");
            let visited = sql(&url, &format!("SELECT count(*) FROM rec_materializable_books WHERE id>={seed} AND id<=(SELECT cursor_book_id FROM rec_generations WHERE id='{generation}'::uuid)")).parse::<u64>().unwrap();
            assert!(visited >= progress.pages && visited <= progress.pages + progress.errors + progress.timeouts);
            let edges = sql(&url, &format!("SELECT count(*) FROM rec_candidate_edges WHERE generation_id='{generation}'::uuid")).parse::<u64>().unwrap();
            assert!(edges <= 70 * visited);
            assert!(store.contextual_status().await.unwrap().held);
            let (status, _) = tokio::time::timeout(GRACE, read(&client, &hidden_path)).await.unwrap().unwrap();
            assert_eq!(status, 404);
            {
                let e = probe.lock().unwrap();
                let ok: Vec<_> = e.iter().filter_map(|(_, value)| *value).collect();
                eprintln!("ADAPT_HTTP_PROGRESS round={} policy={policy:?} visited={visited} remaining={} edges={edges} probe_offered=120 probe_ok={} probe_failed={} probe_unstarted={} probe_p95_us={:?} progress={progress:?} process_cpu_ticks_rss_kb={samples:?}",round+1,total-visited as i64,ok.len(),e.len()-ok.len(),120-e.len(),percentile(ok,95));
            }
            control(&store, Action::Cancel, Some(generation.clone())).await;
            sql(&url, &format!("DELETE FROM rec_generations WHERE id='{generation}'::uuid"));
        }
    }
    server.abort();
    let _ = server.await;
    pool.close().await;
    sql(&url, "DELETE FROM books WHERE source_identity LIKE 'ci:adapt-http:%'");
}
