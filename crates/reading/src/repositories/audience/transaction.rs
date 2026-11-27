//! Transaction setup shared only by the C-owned audience reports.
use super::{unavailable, AudienceError, PgAudience};
use sqlx::{Postgres, Transaction};

#[derive(Clone, Copy)]
pub(super) enum ReportTimeout {
    StatementOnly,
    StatementAndLock,
}

impl PgAudience {
    pub(super) async fn begin_report(
        &self,
        timeout: ReportTimeout,
    ) -> Result<Transaction<'_, Postgres>, AudienceError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        // Preserve the two original SQL statements: SELECT set_config can
        // establish the repeatable-read snapshot, unlike SET LOCAL alone.
        let settings = match timeout {
            ReportTimeout::StatementOnly => "SET LOCAL statement_timeout = '750ms'",
            ReportTimeout::StatementAndLock => {
                "SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)"
            }
        };
        sqlx::query(settings)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::{str::FromStr, time::Duration};

    const SETTINGS: &str = "SELECT current_setting('transaction_isolation'),
        current_setting('transaction_read_only'), current_setting('statement_timeout'),
        current_setting('lock_timeout')";
    type Settings = (String, String, String, String);

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL 18 tor_ci"]
    async fn reuse16a_pg_profiles_and_pool_recovery() {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
        let options = PgConnectOptions::from_str(&url).unwrap();
        assert_eq!(options.get_host(), "127.0.0.1");
        assert_eq!(options.get_database(), Some("tor_ci"));
        assert_eq!(options.get_username(), "tor_app");
        let port = std::env::var("FLOW_TEST_PG_PORT")
            .unwrap_or_else(|_| "5432".into())
            .parse::<u16>()
            .unwrap();
        assert!(port > 0);
        assert_eq!(options.get_port(), port);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await
            .unwrap();
        for statement in [
            "SET default_transaction_isolation = 'read committed'",
            "SET default_transaction_read_only = off",
            "SET statement_timeout = '5s'",
            "SET lock_timeout = '321ms'",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        let baseline = sqlx::query_as::<_, Settings>(SETTINGS)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            baseline,
            (
                "read committed".to_owned(),
                "off".to_owned(),
                "5s".to_owned(),
                "321ms".to_owned(),
            )
        );
        let store = PgAudience::new(pool.clone());
        for (profile, expected_lock) in [
            (ReportTimeout::StatementOnly, "321ms"),
            (ReportTimeout::StatementAndLock, "75ms"),
        ] {
            for should_timeout in [false, true] {
                let mut tx = store.begin_report(profile).await.unwrap();
                let settings = sqlx::query_as::<_, Settings>(SETTINGS)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                assert_eq!(
                    settings,
                    (
                        "repeatable read".to_owned(),
                        "on".to_owned(),
                        "750ms".to_owned(),
                        expected_lock.to_owned(),
                    )
                );
                if should_timeout {
                    let error = sqlx::query("SELECT pg_sleep(2)")
                        .execute(&mut *tx)
                        .await
                        .unwrap_err();
                    let code = error.as_database_error().and_then(|e| e.code());
                    assert_eq!(code.as_deref(), Some("57014"));
                    // Exercise the same drop/rollback path as a failed report.
                    drop(tx);
                } else {
                    tx.commit().await.unwrap();
                }
                // With a single connection, a leaked transaction or session
                // timeout cannot be hidden by a different idle pool connection.
                let restored = sqlx::query_as::<_, Settings>(SETTINGS)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(restored, baseline);
            }
        }
        pool.close().await;
    }
}
