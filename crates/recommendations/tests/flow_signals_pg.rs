//! Real leases, episode transitions, replay and source-scope isolation.
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    str::FromStr,
    time::{Duration, Instant},
};
use tor_recommendations::{repositories::flow_signals::*, services::flow_signals::*};
#[path = "support/flow_fixture.rs"]
mod support;
use support::Fixture;

async fn fixture() -> Fixture {
    let f = Fixture::new(&[
        "flow_signal_rounds",
        "flow_signal_states",
        "flow_signal_events",
    ])
    .await;
    sqlx::query("INSERT INTO flow_signal_rounds(days) VALUES(0),(1),(7),(30),(90)")
        .execute(&f.pool)
        .await
        .unwrap();
    f
}
fn observations(current: Option<i64>, scope: &str) -> Vec<SignalObservation> {
    Rule::ALL
        .into_iter()
        .filter(|r| r.accepts_days(1))
        .map(|rule| SignalObservation {
            rule,
            scope: source_scope(&[scope]),
            window: SignalWindow {
                until_epoch: Some(1_798_000_000),
                label: "current/previous exact owner UTC windows".into(),
            },
            evidence: if rule == Rule::HtmlChange {
                Evidence {
                    available: true,
                    current,
                    previous: Some(100),
                    current_units: 24,
                    previous_units: 24,
                    expected_units: 24,
                    ..Evidence::missing(true)
                }
            } else {
                Evidence::missing(false)
            },
        })
        .collect()
}
async fn save(store: &PgFlowSignals, values: &[SignalObservation]) -> SignalLease {
    let lease = store.claim_round(1).await.unwrap().unwrap();
    store.record_round(lease, values).await.unwrap();
    lease
}
fn html(report: &SignalReport) -> &SignalState {
    report
        .states
        .iter()
        .find(|s| s.rule == Rule::HtmlChange)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn concurrency_replay_unknown_cooldown_and_recovery_are_durable() {
    let f = fixture().await;
    let store = PgFlowSignals::new(f.pool.clone());
    let (a, b) = tokio::join!(store.claim_round(1), store.claim_round(1));
    let leases = [a.unwrap(), b.unwrap()];
    assert_eq!(leases.iter().filter(|l| l.is_some()).count(), 1);
    let lease = leases.into_iter().flatten().next().unwrap();
    let active = observations(Some(50), "source-a");
    store.record_round(lease, &active).await.unwrap();
    store.record_round(lease, &active).await.unwrap();
    let report = store.report(1, None).await.unwrap();
    assert_eq!(report.states.len(), 12);
    assert_eq!(report.events.len(), 1);
    assert_eq!(html(&report).result.status, Status::Active);
    assert_eq!(html(&report).window, active[1].window);
    assert_eq!(
        store
            .record_round(lease, &observations(Some(100), "source-a"))
            .await,
        Err(SignalError::Stale)
    );
    save(&store, &observations(None, "source-a")).await;
    let report = store.report(1, None).await.unwrap();
    assert_eq!(html(&report).result.status, Status::Unknown);
    assert!(html(&report).episode.reason.is_some());
    assert_eq!(report.events.len(), 1);
    save(&store, &observations(Some(100), "source-a")).await;
    let report = store.report(1, None).await.unwrap();
    assert_eq!(report.events.len(), 2);
    assert_eq!(report.events[0].kind, EventKind::Recovered);
    save(&store, &active).await;
    let report = store.report(1, None).await.unwrap();
    assert_eq!(html(&report).result.status, Status::Active);
    assert!(!html(&report).episode.opening_emitted);
    assert_eq!(report.events.len(), 2);
    save(&store, &observations(Some(100), "source-a")).await;
    assert_eq!(
        store.report(1, None).await.unwrap().events.len(),
        2,
        "suppressed opening must not emit orphan recovery"
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn expired_writer_and_changed_scope_do_not_close_or_overwrite_an_episode() {
    let f = fixture().await;
    let store = PgFlowSignals::new(f.pool.clone());
    save(&store, &observations(Some(50), "source-a")).await;
    let old = store.claim_round(1).await.unwrap().unwrap();
    sqlx::query("UPDATE flow_signal_rounds SET lease_until_ms=0 WHERE days=1")
        .execute(&f.pool)
        .await
        .unwrap();
    let current = save(&store, &observations(Some(100), "source-b")).await;
    assert!(current.epoch > old.epoch);
    assert_eq!(
        store
            .record_round(old, &observations(Some(100), "source-a"))
            .await,
        Err(SignalError::Stale)
    );
    let report = store.report(1, None).await.unwrap();
    assert_eq!(html(&report).scope, source_scope(&["source-b"]));
    assert_eq!(
        report.events.len(),
        1,
        "a different source does not recover source-a"
    );
    let old_open: bool = sqlx::query_scalar(
        "SELECT episode_reason IS NOT NULL FROM flow_signal_states WHERE rule='html_change' AND days=1 AND scope=$1"
    ).bind(source_scope(&["source-a"])).fetch_one(&f.pool).await.unwrap();
    assert!(old_open);
    let latest = store.claim_round(0).await.unwrap().unwrap();
    let page = SignalObservation {
        rule: Rule::PublicPageLoss,
        scope: source_scope(&["pages"]),
        window: SignalWindow {
            until_epoch: Some(100),
            label: "latest bounded page sample".into(),
        },
        evidence: Evidence::missing(true),
    };
    store.record_round(latest, &[page]).await.unwrap();
    assert_eq!(
        store.report(90, None).await.unwrap().states.len(),
        1,
        "latest sample has one state across selected periods"
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn reads_are_readonly_stale_safe_and_have_a_pool_inclusive_deadline() {
    let f = fixture().await;
    let store = PgFlowSignals::new(f.pool.clone());
    save(&store, &observations(Some(100), "source-a")).await;
    sqlx::query(
        "UPDATE flow_signal_states SET checked_at_ms=checked_at_ms-301000 WHERE rule='html_change'",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let options = PgConnectOptions::from_str(&f.app_url).unwrap().options([
        ("search_path", format!("{},public", f.schema)),
        ("default_transaction_read_only", "on".into()),
    ]);
    let readonly = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let reader = PgFlowSignals::new(readonly.clone());
    let report = reader.report(1, None).await.unwrap();
    assert_eq!(html(&report).result.status, Status::Healthy);
    assert_eq!(
        html(&report).effective_result(report.checked_at_ms).status,
        Status::Unknown
    );
    let held = readonly.acquire().await.unwrap();
    let started = Instant::now();
    assert_eq!(reader.report(1, None).await, Err(SignalError::Unavailable));
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(held);
    readonly.close().await;
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_signal_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events, 0);
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn changed_active_quality_reason_and_disabled_source_never_emit_recovery() {
    let f = fixture().await;
    let store = PgFlowSignals::new(f.pool.clone());
    let mut values = observations(None, "quality-source");
    for value in &mut values {
        value.evidence = Evidence::missing(false);
    }
    let quality = values
        .iter_mut()
        .find(|v| v.rule == Rule::HtmlQuality)
        .unwrap();
    quality.evidence = Evidence {
        available: true,
        source_failed: true,
        current_units: 24,
        previous_units: 24,
        expected_units: 24,
        ..Evidence::missing(true)
    };
    save(&store, &values).await;
    {
        let quality = values
            .iter_mut()
            .find(|v| v.rule == Rule::HtmlQuality)
            .unwrap();
        quality.evidence.source_failed = false;
        quality.evidence.current_units = 23;
    }
    save(&store, &values).await;
    let report = store.report(1, None).await.unwrap();
    assert_eq!(report.events.len(), 1);
    let quality = report
        .states
        .iter()
        .find(|s| s.rule == Rule::HtmlQuality)
        .unwrap();
    assert_eq!(
        quality.result,
        Evaluation::new(Status::Active, Reason::PartialCoverage)
    );
    assert_eq!(quality.episode.reason, Some(Reason::SourceFailure));
    values
        .iter_mut()
        .find(|v| v.rule == Rule::HtmlQuality)
        .unwrap()
        .evidence
        .enabled = false;
    save(&store, &values).await;
    assert_eq!(store.report(1, None).await.unwrap().events.len(), 1);
    {
        let quality = values
            .iter_mut()
            .find(|v| v.rule == Rule::HtmlQuality)
            .unwrap();
        quality.evidence.enabled = true;
        quality.evidence.current_units = 24;
    }
    save(&store, &values).await;
    let report = store.report(1, None).await.unwrap();
    assert_eq!(report.events.len(), 2);
    assert_eq!(report.events[0].kind, EventKind::Recovered);
    f.cleanup().await;
}
