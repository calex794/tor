//! Independent test entry point; shared real-PG scenario remains unchanged.
use super::paid_scenario;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn retired_funnel_keeps_actual_paid_layer_entitlements() {
    paid_scenario::run().await;
}
