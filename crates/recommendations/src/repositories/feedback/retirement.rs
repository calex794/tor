use super::PgRecommendations;
use crate::domain::feedback::FeedbackError;

impl PgRecommendations {
    /// Drain only the retired analytics store through I's bounded SQL port.
    /// Deleted rows exclude normalized rows, which also spend the SQL budget.
    pub async fn retire_analytics_batch(&self, limit: i32) -> Result<(u64, bool), FeedbackError> {
        if !(1..=1000).contains(&limit) {
            return Err(FeedbackError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        // Both settings precede the function call, including its table locks.
        sqlx::query("SET LOCAL lock_timeout = '250ms'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await?;
        let (erased_rows, pending): (i64, bool) =
            sqlx::query_as("SELECT erased_rows,pending FROM public.rec_retire_analytics_batch($1)")
                .bind(limit)
                .fetch_one(&mut *tx)
                .await?;
        let erased_rows = u64::try_from(erased_rows).map_err(|_| FeedbackError::Unavailable)?;
        if erased_rows > limit as u64 {
            return Err(FeedbackError::Unavailable);
        }
        tx.commit().await?;
        Ok((erased_rows, pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    async fn invalid_budget_is_rejected_without_accessing_the_database() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await;
        let store = PgRecommendations::new(pool);
        for limit in [i32::MIN, -1, 0, 1001, i32::MAX] {
            assert_eq!(
                store.retire_analytics_batch(limit).await,
                Err(FeedbackError::Invalid)
            );
        }
        for limit in [1, 256, 1000] {
            assert_eq!(
                store.retire_analytics_batch(limit).await,
                Err(FeedbackError::Unavailable)
            );
        }
    }
}
