//! Shared setup for F-owned, statement-only read reports. Never used by writers.
use sqlx::{Error, PgPool, Postgres, Transaction};

pub(super) async fn begin_read_report(pool: &PgPool) -> Result<Transaction<'_, Postgres>, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    // Keep SET LOCAL, not SELECT set_config: leave snapshot acquisition and
    // inherited lock_timeout exactly as in the original five report paths.
    sqlx::query("SET LOCAL statement_timeout = '750ms'")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::{str::FromStr, time::Duration};

    const SETTINGS: &str = "SELECT pg_backend_pid(), current_setting('transaction_isolation'),
        current_setting('transaction_read_only'), current_setting('statement_timeout'),
        current_setting('lock_timeout')";
    type Settings = (i32, String, String, String, String);

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL 18 tor_ci"]
    async fn reuse16b_pg_statement_only_and_pool_recovery() {
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
                baseline.0,
                "read committed".to_owned(),
                "off".to_owned(),
                "5s".to_owned(),
                "321ms".to_owned(),
            )
        );
        let mut expected = baseline.clone();
        expected.1 = "repeatable read".to_owned();
        expected.2 = "on".to_owned();
        expected.3 = "750ms".to_owned();

        for cancel in [false, true] {
            let mut tx = begin_read_report(&pool).await.unwrap();
            let settings = sqlx::query_as::<_, Settings>(SETTINGS)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(settings, expected);
            if cancel {
                let error = sqlx::query("SELECT pg_sleep(2)")
                    .execute(&mut *tx)
                    .await
                    .unwrap_err();
                let code = error.as_database_error().and_then(|e| e.code());
                assert_eq!(code.as_deref(), Some("57014"));
                drop(tx);
            } else {
                tx.commit().await.unwrap();
            }
            // The PID and session settings must survive both paths, not just
            // an accidental reconnect with default settings.
            let restored = sqlx::query_as::<_, Settings>(SETTINGS)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(restored, baseline);
        }
        pool.close().await;
    }
}
