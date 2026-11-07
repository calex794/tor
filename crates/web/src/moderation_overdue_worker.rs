//! MOD-OPS-ALERT-01 / #661: opt-in detection ONLY, not notification delivery.
//! A failed or stopped worker cannot destroy observations already in PostgreSQL.
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;

const PERIOD: Duration = Duration::from_secs(60);
const BATCH_LIMIT: u16 = 250;

/// No background moderation scan is allowed by default. It requires a live
/// authenticated PostgreSQL runtime; demo mode cannot silently create signals.
pub(crate) fn worker_opt_in(
    flag: Option<&str>,
    demo: bool,
    has_database: bool,
    has_auth: bool,
) -> Result<bool, &'static str> {
    match flag {
        None | Some("0") => Ok(false),
        Some("1") if !demo && has_database && has_auth => Ok(true),
        Some("1") => Err("MOD overdue observation requires authenticated PostgreSQL (not demo)"),
        _ => Err("TOR_MOD_OVERDUE_OBSERVER_ENABLED must be 0 or 1"),
    }
}

/// Runs only after HTTP listener binding and a successful schema probe.
/// Repeated scan attempts are safe across process restarts and multiple
/// replicas because the repository uses a PostgreSQL transaction advisory
/// lock and immutable source/threshold uniqueness.
///
/// This worker never calls the social notification consumer, mail provider,
/// push endpoint, or the user-facing notification outbox.
pub(crate) fn start(accounts: PgAccounts) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(PERIOD);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            match accounts.scan_moderation_overdue_observations(BATCH_LIMIT).await {
                Ok(result) if result.opened > 0 || result.resolved > 0 => {
                    tracing::info!(
                        opened = result.opened,
                        resolved = result.resolved,
                        "MOD overdue observations reconciled (no delivery)"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "MOD overdue observation scan failed; will retry next tick"
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::worker_opt_in;

    #[test]
    fn fails_closed_without_explicit_operator_opt_in() {
        assert_eq!(worker_opt_in(None, false, true, true), Ok(false));
        assert_eq!(worker_opt_in(Some("0"), false, true, true), Ok(false));
        assert_eq!(worker_opt_in(Some("1"), false, true, true), Ok(true));
    }

    #[test]
    fn invalid_configuration_never_starts_observer() {
        for flag in ["", "yes", "true", "on", "2", " 1"] {
            assert!(worker_opt_in(Some(flag), false, true, true).is_err());
        }
        for (demo, database, auth) in [
            (true, true, true),
            (false, false, true),
            (false, true, false),
            (true, false, false),
        ] {
            assert!(worker_opt_in(Some("1"), demo, database, auth).is_err());
        }
    }
}
