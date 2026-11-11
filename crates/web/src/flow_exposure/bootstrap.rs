use super::{ObservationCollector, Runtime};
use axum::{http::Uri, Router};
use tokio::task::JoinHandle;
use tor_accounts::repositories::PgAccounts;

/// Public AUTH=0 installations without an origin remain uninstrumented. The
/// authenticated production branch already validates this same origin in main.
pub async fn start(app: Router, accounts: Option<PgAccounts>) -> (Router, Option<JoinHandle<()>>) {
    let (app, collector) = start_managed(app, accounts).await;
    (app, collector.map(|c| c.join))
}

pub async fn start_managed(
    app: Router,
    accounts: Option<PgAccounts>,
) -> (Router, Option<ObservationCollector>) {
    let Some(accounts) = accounts else {
        return (app, None);
    };
    let Ok(origin) = std::env::var("TOR_PUBLIC_ORIGIN") else {
        return (app, None);
    };
    let Ok(uri) = origin.parse::<Uri>() else {
        return (app, None);
    };
    let local = matches!(uri.host(), Some("localhost" | "127.0.0.1"));
    if !(uri.scheme_str() == Some("https") || uri.scheme_str() == Some("http") && local)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || uri.path_and_query().is_some_and(|p| p.as_str() != "/")
        || origin.ends_with('/')
    {
        return (app, None);
    }
    let secure = origin.starts_with("https://");
    let guard = crate::editorial_http::observation_guard(&accounts);
    let (runtime, mut worker) = Runtime::new(accounts, origin, secure, Some(guard));
    let enabled = !std::env::var("TOR_FLOW_OBSERVATIONS_ENABLED")
        .is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"));
    if !enabled || !worker.store.available().await || !worker.health.available().await {
        return (runtime.attach_routes(app), None);
    }
    if worker.prepare().await.is_err() {
        return (runtime.attach_routes(app), None);
    }
    let probe = runtime.probe();
    let (stop, receiver) = tokio::sync::oneshot::channel();
    let join = tokio::spawn(worker.run_managed(receiver));
    (
        runtime.attach(app),
        Some(ObservationCollector { join, stop, probe }),
    )
}
