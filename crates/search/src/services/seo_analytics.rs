//! Bounded external-search ingestion. A lease is not a request for a user identity.
use crate::domain::seo_analytics::*;
use async_trait::async_trait;
#[async_trait]
pub trait SearchAnalyticsProvider: Send + Sync {
    async fn collect(&self, day: &str) -> Result<Snapshot, SeoAnalyticsError>;
}
#[async_trait]
pub trait SeoAnalyticsStore: Send + Sync {
    async fn claim(&self, source: &Source) -> Result<Option<Job>, SeoAnalyticsError>;
    async fn finish(&self, job: &Job, snapshot: &Snapshot) -> Result<(), SeoAnalyticsError>;
    async fn fail(&self, job: &Job, error: SeoAnalyticsError) -> Result<(), SeoAnalyticsError>;
}
/// Network time is outside any database transaction. Stale completions cannot
/// replace a newer successful snapshot. A failed refresh retains the old data.
pub async fn sync_one(
    store: &dyn SeoAnalyticsStore,
    provider: &dyn SearchAnalyticsProvider,
    source: &Source,
) -> Result<bool, SeoAnalyticsError> {
    let Some(job) = store.claim(source).await? else {
        return Ok(false);
    };
    let result = match tokio::time::timeout(
        std::time::Duration::from_secs(100),
        provider.collect(&job.day),
    )
    .await
    {
        Ok(Ok(snapshot)) => match snapshot.validate(&job.source.key.origin) {
            Ok(()) => Ok(snapshot),
            Err(e) => Err(e),
        },
        Ok(Err(e)) => Err(e),
        Err(_) => Err(SeoAnalyticsError::Unavailable),
    };
    match result {
        Ok(snapshot) => store.finish(&job, &snapshot).await?,
        Err(error) => {
            store.fail(&job, error).await?;
            return Err(error);
        }
    }
    Ok(true)
}
