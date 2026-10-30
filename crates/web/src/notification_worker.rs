//! Local development-only G-08 worker runner, explicitly opted in.
//! Disabled in demo, on non-local databases, and by default. No email/push.
use std::time::Duration;
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tor_books::repositories::PgCatalog;
use tor_ingest::{
    repositories::{notify::PgNotificationRecipients, outbox::PgOutbox},
    services::notify_worker::{NotificationRun, NotificationWorker},
};

fn disposable_local_database(url: &str) -> bool {
    let Some(value) = url.strip_prefix("postgresql://tor_app:") else {
        return false;
    };
    let Some((_, host_and_db)) = value.rsplit_once('@') else {
        return false;
    };
    let Some(host) = host_and_db.strip_suffix("/tor_ci") else {
        return false;
    };
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    for prefix in ["localhost:", "127.0.0.1:", "[::1]:"] {
        if let Some(port) = host.strip_prefix(prefix) {
            return port.parse::<u16>().is_ok_and(|port| port > 0);
        }
    }
    false
}

/// Only TOR_DEV_NOTIFICATION_WORKER_ENABLED=1 with a disposable local tor_ci
/// database can activate polling. Invalid values fail startup rather than
/// silently enabling jobs.
pub fn worker_opt_in(
    setting: Option<&str>,
    demo: bool,
    database_url: Option<&str>,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1") => {
            if demo || !database_url.is_some_and(disposable_local_database) {
                Err("Development G-08 worker requires local tor_app /tor_ci and non-demo mode")
            } else {
                Ok(true)
            }
        }
        Some(_) => Err("TOR_DEV_NOTIFICATION_WORKER_ENABLED must be 0 or 1"),
    }
}

/// One bounded run per five-second tick, sharing the web server's PG pool.
/// On timeout stop rather than start overlapping potentially leased work.
pub fn start_dev_notification_worker_background(catalog: PgCatalog) -> JoinHandle<()> {
    let pool = catalog.pool();
    let worker = NotificationWorker::new(
        PgOutbox::new(pool.clone()),
        PgNotificationRecipients::new(pool),
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match tokio::time::timeout(Duration::from_secs(50), worker.run_one()).await {
                Ok(Ok(NotificationRun::Idle | NotificationRun::Superseded)) => {}
                Ok(Ok(NotificationRun::Staged { recipients, pages })) => {
                    tracing::info!(
                        recipients,
                        pages,
                        "development notification metadata staging complete"
                    );
                }
                Ok(Ok(NotificationRun::RetryScheduled)) => {
                    tracing::warn!("development notification staging scheduled retry");
                }
                Ok(Ok(NotificationRun::DeadLetter)) => {
                    tracing::warn!("development notification staging dead-lettered");
                }
                Ok(Err(error)) => {
                    tracing::warn!(?error, "development notification database unavailable");
                }
                Err(_) => {
                    tracing::error!("development notification runner timed out; stopped");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_local_disposable_opt_in_only() {
        assert_eq!(worker_opt_in(None, false, None), Ok(false));
        assert_eq!(worker_opt_in(Some("0"), false, None), Ok(false));
        assert_eq!(
            worker_opt_in(
                Some("1"),
                false,
                Some("postgresql://tor_app:test@127.0.0.1:5432/tor_ci"),
            ),
            Ok(true)
        );
        for url in [
            "postgresql://tor_app:test@db.internal:5432/tor_ci",
            "postgresql://tor_app:test@localhost.evil.test/tor_ci",
            "postgresql://tor_app:test@127.0.0.1:5432/tor_prod",
            "postgresql://tor_app:test@127.0.0.1:0/tor_ci",
            "postgresql://tor_app:test@127.0.0.1:noport/tor_ci",
            "postgresql://tor_app:test@127.0.0.1:5432/tor_ci?sslmode=require",
            "postgresql://admin:test@127.0.0.1:5432/tor_ci",
        ] {
            assert!(worker_opt_in(Some("1"), false, Some(url)).is_err());
        }
        assert!(worker_opt_in(Some("1"), false, None).is_err());
        assert!(worker_opt_in(Some("1"), true, None).is_err());
        for invalid in ["true", "01", "2", "", "yes"] {
            assert!(worker_opt_in(Some(invalid), false, None).is_err());
        }
    }
}
