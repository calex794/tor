//! I-owned configuration and scheduling; C owns collection and cohort decisions.
use std::{env, time::Duration};
use tokio::{task::JoinHandle, time::sleep};
use tor_reading::repositories::returns::PgReadingReturns;
use tor_reading::services::returns::ReadingReturnsError;

pub fn reading_returns_enabled() -> bool {
    [
        "TOR_READING_RETURNS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
    ]
    .iter()
    .all(|name| env::var(name).as_deref() == Ok("1"))
}

pub fn validate_reading_returns_runtime(demo: bool) -> Result<bool, &'static str> {
    let enabled = match env::var("TOR_READING_RETURNS_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_READING_RETURNS_ENABLED must be 0 or 1"),
    };
    if enabled
        && (demo
            || env::var_os("DATABASE_URL").is_none()
            || env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1")
            || !reading_returns_enabled())
    {
        return Err("Reading returns require authenticated PostgreSQL and enabled REC collection");
    }
    Ok(enabled)
}

/// Start even when collection is off: close old coverage and expire private state.
/// First progress waits for a successful heartbeat; startup never backfills history.
pub async fn spawn_reading_returns(
    store: Option<PgReadingReturns>,
    collect: bool,
) -> Result<Option<JoinHandle<()>>, ReadingReturnsError> {
    let Some(store) = store else {
        return Ok(None);
    };
    // Complete the new coverage boundary before HTTP can accept any progress.
    store.start_collection(collect).await?;
    Ok(Some(tokio::spawn(async move {
        loop {
            if store.tick_collection(collect).await.is_err() {
                eprintln!("reading_returns_worker: batch failed; coverage will expire");
            }
            sleep(Duration::from_secs(5)).await;
        }
    })))
}
