//! Optional operational source. It creates no page-view, reader or observation facts.
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;
use tor_search::{
    domain::public_pages::{PageProbeError, PublicPageReport, PROBE_INTERVAL_SECONDS},
    repositories::public_pages::PgPublicPages,
    services::public_pages::PublicPageProbe,
};

pub fn enabled() -> bool {
    std::env::var("TOR_FLOW_PUBLIC_PAGES_ENABLED").as_deref()!=Ok("0")
}
pub fn configured() -> Result<PublicPageProbe, PageProbeError> {
    PublicPageProbe::new(&std::env::var("TOR_PUBLIC_ORIGIN").map_err(|_| PageProbeError::Unavailable)?)
}
pub async fn report(accounts: &PgAccounts) -> Result<PublicPageReport, PageProbeError> {
    let probe = configured()?;
    PgPublicPages::new(accounts.pool().clone()).report(probe.origin()).await
}
pub fn start(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() { return None; }
    let accounts = accounts?;
    let probe = configured().ok()?;
    let store = PgPublicPages::new(accounts.pool().clone());
    Some(tokio::spawn(async move {
        loop {
            if probe.check_once(&store).await.is_err() { eprintln!("TOR_FLOW_PUBLIC_PAGES_UNAVAILABLE"); }
            tokio::time::sleep(Duration::from_secs(PROBE_INTERVAL_SECONDS)).await;
        }
    }))
}
