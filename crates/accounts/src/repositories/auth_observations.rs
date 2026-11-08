//! Private A-09 auth-observation persistence. Never query this in a public route.
use super::PgAccounts;
use crate::domain::auth_observation::AuthAttemptObservation;

impl PgAccounts {
    pub async fn record_auth_observation(
        &self,
        event: &AuthAttemptObservation,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO auth_login_observations \
             (outcome, target_key, user_id, source_key, subnet_key, agent_family) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(event.outcome.label())
        .bind(event.target_key.map(|value| value.to_vec()))
        .bind(event.user_id)
        .bind(event.source_key.map(|value| value.to_vec()))
        .bind(event.subnet_key.map(|value| value.to_vec()))
        .bind(event.agent.label())
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
