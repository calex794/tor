//! ADAPT-02c: actual repository calls in the existing disposable 100k fixture.
//! The offered workload and thresholds are experimental; no runtime change.
use super::*;
use std::sync::Mutex;
use tokio::time::{sleep_until, timeout_at, Instant};

const RUN: Duration = Duration::from_secs(8);
const GRACE: Duration = Duration::from_secs(2);
const SAMPLE_STEP: Duration = Duration::from_millis(100);
const FRESH: Duration = Duration::from_secs(1);
const MIN_MS: u64 = 250;
const MAX_MS: u64 = 5000;

#[derive(Clone, Copy, Debug)]
enum Policy {
    Baseline,
    AgeAware,
    ProgressBudget,
}

#[derive(Clone, Copy, Debug)]
enum Pressure {
    Missing,
    High,
    Low,
    Neutral,
}

struct Pacer {
    policy: Policy,
    delay: u64,
    healthy: u8,
}

impl Pacer {
    fn new(policy: Policy) -> Self {
        Self {
            policy,
            delay: MIN_MS,
            healthy: 0,
        }
    }

    fn after_progress(&mut self, pressure: Pressure, pending: i64, age: Duration) -> u64 {
        if matches!(self.policy, Policy::Baseline) {
            return MIN_MS;
        }
        match pressure {
            Pressure::Missing => {
                self.delay = MAX_MS;
                self.healthy = 0;
            }
            Pressure::High => {
                let ceiling = if matches!(self.policy, Policy::ProgressBudget) {
                    self.delay.max(1000)
                } else {
                    MAX_MS
                };
                self.delay = (2 * self.delay).min(ceiling);
                self.healthy = 0;
            }
            Pressure::Low => {
                self.healthy += 1;
                if pending >= 8 || age >= Duration::from_secs(8) {
                    self.delay = MIN_MS;
                    self.healthy = 0;
                } else if self.healthy >= 2 {
                    self.delay = (self.delay / 2).max(MIN_MS);
                    self.healthy = 0;
                }
            }
            Pressure::Neutral => self.healthy = 0,
        }
        assert!((MIN_MS..=MAX_MS).contains(&self.delay));
        self.delay
    }
}

#[derive(Clone, Copy)]
struct Observation {
    at: Instant,
    micros: Option<u64>,
}

#[derive(Default)]
struct Evidence {
    reader: Vec<Observation>,
    service_us: Vec<u64>,
    probe: Vec<Observation>,
    reader_errors: usize,
    reader_timeouts: usize,
    reader_unstarted: usize,
    probe_errors: usize,
    probe_timeouts: usize,
    probe_unstarted: usize,
}

fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).expect("bounded trial duration")
}

fn percentile(mut values: Vec<u64>, p: usize) -> Option<u64> {
    values.sort_unstable();
    let rank = (values.len() * p).div_ceil(100);
    rank.checked_sub(1).and_then(|i| values.get(i).copied())
}

fn recent(samples: &[Observation], now: Instant) -> Vec<Option<u64>> {
    samples
        .iter()
        .filter(|sample| now.saturating_duration_since(sample.at) <= FRESH)
        .map(|sample| sample.micros)
        .collect()
}

fn pressure(evidence: &Evidence, now: Instant) -> Pressure {
    let reader = recent(&evidence.reader, now);
    let probe = recent(&evidence.probe, now);
    if reader.is_empty() || probe.is_empty() {
        return Pressure::Missing;
    }
    if reader.iter().chain(&probe).any(Option::is_none) {
        return Pressure::High;
    }
    let reader_p95 = percentile(reader.into_iter().flatten().collect(), 95).unwrap();
    let probe_p95 = percentile(probe.into_iter().flatten().collect(), 95).unwrap();
    if reader_p95 >= 100_000 || probe_p95 >= 50_000 {
        Pressure::High
    } else if reader_p95 <= 50_000 && probe_p95 <= 10_000 {
        Pressure::Low
    } else {
        Pressure::Neutral
    }
}

/// The base lane offers 80 reads at 100 ms intervals. Three additional lanes
/// offer 30 each during seconds 2..5. A delayed lane cannot silently skip
/// requests: all unstarted requests are counted at the common grace deadline.
async fn reader_lane(
    store: &PgRecommendations,
    seed: i64,
    expected: &[i64],
    start: Instant,
    lane: usize,
    evidence: &Mutex<Evidence>,
) {
    let hard_end = start + RUN + GRACE;
    let (first, last) = if lane == 0 { (0, 80) } else { (20, 50) };
    for tick in first..last {
        let due = start + SAMPLE_STEP * tick;
        sleep_until(due).await;
        let begun = Instant::now();
        if begun >= hard_end {
            evidence.lock().unwrap().reader_unstarted += 1;
            continue;
        }
        let deadline = (begun + GRACE).min(hard_end);
        let result = timeout_at(deadline, store.feed(None, Some(seed), 20)).await;
        let now = Instant::now();
        let mut state = evidence.lock().unwrap();
        let sample = match result {
            Ok(Ok(feed)) => {
                assert_catalog(&feed);
                assert_eq!(ids(&feed).as_slice(), expected);
                state.service_us.push(micros(now - begun));
                Some(micros(now - due))
            }
            Ok(Err(_)) => {
                state.reader_errors += 1;
                None
            }
            Err(_) => {
                state.reader_timeouts += 1;
                None
            }
        };
        state.reader.push(Observation {
            at: now,
            micros: sample,
        });
    }
}

/// Independent timed acquire/release probe on the SAME pool. This measures
/// probe wait, not internal SQLx wait of each individual feed transaction.
async fn pool_probe(pool: &PgPool, start: Instant, evidence: &Mutex<Evidence>) {
    let hard_end = start + RUN + GRACE;
    for tick in 0..40 {
        sleep_until(start + Duration::from_millis(200) * tick).await;
        let begun = Instant::now();
        if begun >= hard_end {
            evidence.lock().unwrap().probe_unstarted += 1;
            continue;
        }
        let result = timeout_at((begun + GRACE).min(hard_end), pool.acquire()).await;
        let now = Instant::now();
        let mut state = evidence.lock().unwrap();
        let sample = match result {
            Ok(Ok(connection)) => {
                drop(connection);
                Some(micros(now - begun))
            }
            Ok(Err(_)) => {
                state.probe_errors += 1;
                None
            }
            Err(_) => {
                state.probe_timeouts += 1;
                None
            }
        };
        state.probe.push(Observation {
            at: now,
            micros: sample,
        });
    }
}

#[derive(Default, Debug)]
struct BuildEvidence {
    pages: i64,
    errors: i64,
    timeouts: i64,
    idle: i64,
    decisions: [usize; 4],
    max_gap_us: u64,
    delays_ms: Vec<u64>,
}

async fn builder(
    store: &PgRecommendations,
    start: Instant,
    total: i64,
    policy: Policy,
    evidence: &Mutex<Evidence>,
) -> BuildEvidence {
    let end = start + RUN;
    let mut pacer = Pacer::new(policy);
    let mut result = BuildEvidence::default();
    let mut last_progress = start;
    sleep_until(start).await;
    while Instant::now() < end {
        let attempt = timeout_at(
            Instant::now() + GRACE,
            store.advance_contextual_generation(),
        )
        .await;
        let now = Instant::now();
        let delay = match attempt {
            Ok(Ok(true)) => {
                result.pages += 1;
                result.max_gap_us = result.max_gap_us.max(micros(now - last_progress));
                last_progress = now;
                let signal = pressure(&evidence.lock().unwrap(), now);
                let index = match signal {
                    Pressure::Missing => 0,
                    Pressure::High => 1,
                    Pressure::Low => 2,
                    Pressure::Neutral => 3,
                };
                result.decisions[index] += 1;
                pacer.after_progress(signal, total.saturating_sub(result.pages), now - start)
            }
            Ok(Ok(false)) => {
                result.idle += 1;
                MAX_MS
            }
            Ok(Err(_)) => {
                result.errors += 1;
                MAX_MS
            }
            Err(_) => {
                result.timeouts += 1;
                MAX_MS
            }
        };
        result.delays_ms.push(delay);
        sleep_until((now + Duration::from_millis(delay)).min(end)).await;
    }
    result.max_gap_us = result
        .max_gap_us
        .max(micros(Instant::now() - last_progress));
    result
}

async fn trial(f: &Fixture, pool: &PgPool, sentinel: i64, policy: Policy, round: usize) {
    let seed = f.books[0];
    let generation = command(f, Action::Refresh, None).await;
    sqlx::query("UPDATE rec_generations SET cursor_book_id=$2 WHERE id=$1::uuid")
        .bind(&generation)
        .bind(seed - 1)
        .execute(&f.pool)
        .await
        .unwrap();
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rec_materializable_books WHERE id>$1")
            .bind(seed - 1)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(total >= 100_000, "actual materializable cohort");
    let store = PgRecommendations::new(pool.clone());
    let feed = store.feed(None, Some(seed), 20).await.unwrap();
    assert_catalog(&feed);
    let expected = ids(&feed);
    assert!(!expected.is_empty());
    let evidence = Mutex::new(Evidence::default());
    let start = Instant::now() + Duration::from_millis(100);
    let (build, _, _, _, _, _) = tokio::join!(
        builder(&store, start, total, policy, &evidence),
        reader_lane(&store, seed, &expected, start, 0, &evidence),
        reader_lane(&store, seed, &expected, start, 1, &evidence),
        reader_lane(&store, seed, &expected, start, 2, &evidence),
        reader_lane(&store, seed, &expected, start, 3, &evidence),
        pool_probe(pool, start, &evidence),
    );
    let state = evidence.into_inner().unwrap();
    let reader_us: Vec<_> = state.reader.iter().filter_map(|s| s.micros).collect();
    let probe_us: Vec<_> = state.probe.iter().filter_map(|s| s.micros).collect();
    assert_eq!(
        reader_us.len() + state.reader_errors + state.reader_timeouts + state.reader_unstarted,
        170
    );
    assert_eq!(
        probe_us.len() + state.probe_errors + state.probe_timeouts + state.probe_unstarted,
        40
    );

    // Authoritative cursor, not a count of successful client responses alone.
    let (cursor, status): (i64, String) =
        sqlx::query_as("SELECT cursor_book_id,status FROM rec_generations WHERE id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(status, "building");
    let visited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rec_materializable_books WHERE id>$1 AND id<=$2")
            .bind(seed - 1)
            .bind(cursor)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    // A cancelled response could race an already committed page. Expose that
    // difference rather than claiming that every timeout means no progress.
    assert!(visited >= build.pages && visited <= build.pages + build.errors + build.timeouts);
    let edges: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rec_candidate_edges WHERE generation_id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(edges <= 70 * visited);
    assert_eq!(
        store.feed(None, Some(sentinel), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    assert!(store.contextual_status().await.unwrap().held);

    eprintln!(
        "ADAPT_PG round={round} policy={policy:?} reader_offered=170 reader_ok={} \
         reader_errors={} reader_timeouts={} reader_unstarted={} reader_p95_us={:?} \
         reader_p99_us={:?} service_p95_us={:?} probe_offered=40 probe_ok={} \
         probe_errors={} probe_timeouts={} probe_unstarted={} probe_p95_us={:?} \
         materializable={total} visited={visited} remaining={} edges={edges} build={build:?}",
        reader_us.len(),
        state.reader_errors,
        state.reader_timeouts,
        state.reader_unstarted,
        percentile(reader_us.clone(), 95),
        percentile(reader_us, 99),
        percentile(state.service_us, 95),
        probe_us.len(),
        state.probe_errors,
        state.probe_timeouts,
        state.probe_unstarted,
        percentile(probe_us, 95),
        total - visited,
    );
    command(f, Action::Cancel, Some(&generation)).await;
    stop(f, &[generation]).await;
}

pub(super) async fn run(f: &Fixture, pool: &PgPool, sentinel: i64) {
    pacing_never_accelerates_on_high_or_missing_pressure();
    // Reverse order in round 2 to expose warm-cache and runner-order effects.
    for (round, order) in [
        [Policy::Baseline, Policy::AgeAware, Policy::ProgressBudget],
        [Policy::ProgressBudget, Policy::AgeAware, Policy::Baseline],
    ]
    .into_iter()
    .enumerate()
    {
        for policy in order {
            trial(f, pool, sentinel, policy, round + 1).await;
        }
    }
}

fn pacing_never_accelerates_on_high_or_missing_pressure() {
    for policy in [Policy::AgeAware, Policy::ProgressBudget] {
        let mut pacer = Pacer::new(policy);
        let mut last = MIN_MS;
        for _ in 0..20 {
            let delay = pacer.after_progress(Pressure::High, 100_000, RUN);
            assert!((last..=MAX_MS).contains(&delay));
            last = delay;
        }
        assert_eq!(
            pacer.after_progress(Pressure::Missing, 100_000, RUN),
            MAX_MS
        );
        assert_eq!(pacer.after_progress(Pressure::High, 100_000, RUN), MAX_MS);
        assert_eq!(pacer.after_progress(Pressure::Low, 100_000, RUN), MIN_MS);
    }
}
