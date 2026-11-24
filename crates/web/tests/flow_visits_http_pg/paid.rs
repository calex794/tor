//! Independent test entry point; shared real-PG scenario remains unchanged.
use super::paid_scenario;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn paid_and_layer_views_need_actual_entitlements_and_live_session() {
    paid_scenario::run().await;
}
