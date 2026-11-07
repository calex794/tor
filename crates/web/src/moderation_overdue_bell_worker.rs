//! #661: explicitly opted-in on-site bell projection. No email/SMS/push.
//! Recipient class and 24h/72h policy must be separately accepted in the
//! process environment; this task is never enabled by defaults or migrations.
use std::time::Duration;
use tokio::time::{interval_at, Instant, MissedTickBehavior};
use tor_social::repositories::notifications::PgNotifications;

const INTERVAL: Duration = Duration::from_secs(60);
const INITIAL_DELAY: Duration = Duration::from_secs(15);
const BATCH_LIMIT: u16 = 100;

pub(crate) fn read_setting(name: &str) -> Result<Option<String>, std::env::VarError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn worker_opt_in(
    enabled: Option<&str>,
    recipients: Option<&str>,
    thresholds: Option<&str>,
    observer_enabled: bool,
    demo: bool,
    has_database: bool,
    has_auth: bool,
) -> Result<bool, &'static str> {
    match enabled {
        None | Some("0") if recipients.is_none() && thresholds.is_none() => Ok(false),
        None | Some("0") => Err(
            "MOD overdue bell policy supplied while delivery is disabled",
        ),
        Some("1")
            if recipients == Some("all-active-site-administrators")
                && thresholds == Some("elapsed-hours-24-72")
                && observer_enabled
                && !demo
                && has_database
                && has_auth =>
        {
            Ok(true)
        }
        Some("1") => Err(
            "MOD overdue bell delivery requires explicit administrator recipients, elapsed 24/72h policy, and authenticated PostgreSQL observation",
        ),
        _ => Err("TOR_MOD_OVERDUE_BELL_DELIVERY_ENABLED must be 0 or 1"),
    }
}

/// Bounded internal-only delivery to existing user_notifications. The
/// repository transaction claims, projects and receipts each unique
/// signal/recipient pair. It rechecks live source + administrator roles,
/// and the read path rechecks them again before surfacing a bell.
pub(crate) fn start(repo: PgNotifications) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Allow the observation worker's immediate tick to populate the
        // ledger first. Both workers are serialized internally by PostgreSQL.
        let mut tick = interval_at(Instant::now() + INITIAL_DELAY, INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match repo.project_moderation_overdue_to_admin_bell(BATCH_LIMIT).await {
                Ok(projected) if projected > 0 => {
                    tracing::info!(
                        projected,
                        "MOD site-administrator overdue bell observations projected"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    // No recipient IDs, Book titles, message bodies or media
                    // access paths are logged by this worker.
                    tracing::warn!(?error, "MOD overdue bell projection unavailable");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::worker_opt_in;

    #[test]
    fn disabled_is_always_the_default() {
        assert_eq!(worker_opt_in(None, None, None, false, false, false, false), Ok(false));
        assert_eq!(
            worker_opt_in(Some("0"), None, None, true, false, true, true),
            Ok(false)
        );
    }

    #[test]
    fn requires_explicit_approved_recipient_and_threshold_policies() {
        let accepted = (
            Some("1"),
            Some("all-active-site-administrators"),
            Some("elapsed-hours-24-72"),
        );
        assert_eq!(
            worker_opt_in(accepted.0, accepted.1, accepted.2, true, false, true, true),
            Ok(true)
        );
        for (observer, demo, database, auth) in [
            (false, false, true, true),
            (true, true, true, true),
            (true, false, false, true),
            (true, false, true, false),
        ] {
            assert!(worker_opt_in(
                accepted.0, accepted.1, accepted.2, observer, demo, database, auth
            )
            .is_err());
        }
        assert!(worker_opt_in(
            accepted.0,
            Some("all-site-moderators"),
            accepted.2,
            true,
            false,
            true,
            true
        )
        .is_err());
        assert!(worker_opt_in(
            accepted.0,
            accepted.1,
            Some("local-business-hours"),
            true,
            false,
            true,
            true
        )
        .is_err());
    }

    #[test]
    fn rejects_partial_or_unexpected_operator_settings() {
        for invalid in ["", "true", "yes", "2", " 1"] {
            assert!(worker_opt_in(
                Some(invalid), None, None, true, false, true, true
            )
            .is_err());
        }
        assert!(worker_opt_in(
            None, Some("all-active-site-administrators"), None, true, false, true, true
        )
        .is_err());
        assert!(worker_opt_in(
            Some("0"), None, Some("elapsed-hours-24-72"), true, false, true, true
        )
        .is_err());
        assert!(worker_opt_in(
            Some("1"), None, None, true, false, true, true
        )
        .is_err());
    }
}
