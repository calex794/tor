use super::PgExposure;
use std::time::Duration;

impl PgExposure {
    /// Startup capability check only; no issuer should be exposed without the
    /// independent observation schema. Legacy analytics fixtures remain untouched.
    /// The deadline includes obtaining a connection, not only the SQL statements.
    pub async fn available(&self) -> bool {
        let probe = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET TRANSACTION READ ONLY")
                .execute(&mut *tx)
                .await?;
            sqlx::query("SELECT set_config('statement_timeout','200ms',true),set_config('lock_timeout','25ms',true)").execute(&mut *tx).await?;
            let installed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611286493 AND success) AND to_regclass('public.flow_observation_state') IS NOT NULL AND to_regclass('public.flow_observation_proofs') IS NOT NULL AND to_regclass('public.flow_observation_hourly') IS NOT NULL AND EXISTS(SELECT 1 FROM pg_catalog.pg_trigger WHERE tgrelid=to_regclass('public.flow_observation_proofs') AND tgname='flow_observation_proof_immutable' AND tgenabled IN ('O','A') AND NOT tgisinternal)").fetch_one(&mut *tx).await?;
            let initialized = if installed {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM public.flow_observation_state WHERE singleton)",
                )
                .fetch_one(&mut *tx)
                .await?
            } else {
                false
            };
            tx.commit().await?;
            Ok::<bool, sqlx::Error>(initialized)
        };
        matches!(
            tokio::time::timeout(Duration::from_millis(300), probe).await,
            Ok(Ok(true))
        )
    }
}
