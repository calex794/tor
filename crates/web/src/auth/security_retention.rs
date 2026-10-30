use tor_accounts::repositories::PgAccounts;

pub(super) fn start(accounts: PgAccounts) {
    // Router construction remains possible outside Tokio in static unit checks.
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(300));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if accounts.prune_security_data().await.is_err() {
                eprintln!("TOR_ACCOUNT_RETENTION_UNAVAILABLE");
            }
        }
    });
}
