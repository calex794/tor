use super::Runtime;
use axum::{http::Uri, Router};
use tokio::task::JoinHandle;
use tor_accounts::repositories::PgAccounts;

/// Public AUTH=0 installations without an origin remain uninstrumented. The
/// authenticated production branch already validates this same origin in main.
pub async fn start(app: Router, accounts: Option<PgAccounts>) -> (Router, Option<JoinHandle<()>>) {
    let Some(accounts) = accounts else {
        return (app, None);
    };
    if !tor_recommendations::repositories::exposure::PgExposure::new(accounts.pool().clone())
        .available()
        .await
    {
        return (app, None);
    }
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
    let (runtime, worker) = Runtime::new(accounts, origin, secure, None);
    (runtime.attach(app), Some(tokio::spawn(worker.run())))
}
