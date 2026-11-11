//! Bounded background composition of aggregate owner reports. GET only reads the journal.
use std::time::Duration;
use tokio::task::JoinHandle;
use tor_accounts::repositories::PgAccounts;
use tor_reading::repositories::audience::PgAudience;
use tor_recommendations::{
    repositories::{exposure::PgExposure, flow_signals::PgFlowSignals},
    services::flow_signals::SignalError,
};
use tor_search::{
    domain::seo_analytics::Provider,
    repositories::{public_pages::PgPublicPages, seo_analytics::PgSeoAnalytics},
    services::public_pages::PublicPageProbe,
};
use tor_traffic_behavior::repositories::PgFlowAudience;
mod evidence;
mod providers;

const OWNER_BUDGET: Duration = Duration::from_millis(950);

#[derive(Clone, Debug, Default)]
pub struct SignalSources {
    pub origin: Option<String>,
    pub observations_enabled: bool,
    pub seo_enabled: bool,
    pub seo_origin: Option<String>,
    pub google_property: Option<String>,
    pub yandex_property: Option<String>,
}
impl SignalSources {
    fn from_environment() -> Self {
        Self {
            origin: std::env::var("TOR_PUBLIC_ORIGIN").ok(),
            observations_enabled: !std::env::var("TOR_FLOW_OBSERVATIONS_ENABLED")
                .is_ok_and(|value| matches!(value.as_str(), "0" | "false" | "off")),
            seo_enabled: std::env::var("TOR_FLOW_SEO_IMPORT").as_deref() == Ok("1"),
            seo_origin: std::env::var("TOR_FLOW_SEO_ORIGIN").ok(),
            google_property: std::env::var("TOR_FLOW_GOOGLE_PROPERTY").ok(),
            yandex_property: std::env::var("TOR_FLOW_YANDEX_PROPERTY").ok(),
        }
    }
}

#[derive(Clone)]
pub struct SignalWorker {
    journal: PgFlowSignals,
    html: PgFlowAudience,
    reading: PgAudience,
    exposure: PgExposure,
    seo: PgSeoAnalytics,
    pages: PgPublicPages,
    sources: SignalSources,
}
impl SignalWorker {
    pub fn new(accounts: &PgAccounts, sources: SignalSources) -> Self {
        let pool = accounts.pool().clone();
        Self {
            journal: PgFlowSignals::new(pool.clone()),
            html: PgFlowAudience::new(pool.clone()),
            reading: PgAudience::new(pool.clone()),
            exposure: PgExposure::new(pool.clone()),
            seo: PgSeoAnalytics::new(pool.clone()),
            pages: PgPublicPages::new(pool),
            sources,
        }
    }
    pub async fn run_once(&self, days: i32) -> Result<bool, SignalError> {
        if !matches!(days, 1 | 7 | 30 | 90) {
            return Err(SignalError::Invalid);
        }
        tokio::time::timeout(Duration::from_secs(5), self.round(days))
            .await
            .map_err(|_| SignalError::Unavailable)?
    }
    async fn round(&self, days: i32) -> Result<bool, SignalError> {
        let Some(lease) = self.journal.claim_round(days).await? else {
            return Ok(false);
        };
        // At most two owner reads in parallel. No SQL connection is held while
        // reading another owner or doing public-page/provider network work.
        let (html, reading) = tokio::join!(
            tokio::time::timeout(OWNER_BUDGET, self.html.observed_health_report(days, "UTC")),
            tokio::time::timeout(OWNER_BUDGET, self.reading.closed_report(days)),
        );
        let mut observations = evidence::html(days, html.ok().and_then(Result::ok));
        observations.extend(evidence::reading(days, reading.ok().and_then(Result::ok)));
        let (google, yandex) = tokio::join!(
            tokio::time::timeout(OWNER_BUDGET, self.seo.report(days, Provider::Google, None)),
            tokio::time::timeout(OWNER_BUDGET, self.seo.report(days, Provider::Yandex, None)),
        );
        observations.extend(providers::seo(
            days,
            Provider::Google,
            &self.sources,
            google.ok().and_then(Result::ok),
        ));
        observations.extend(providers::seo(
            days,
            Provider::Yandex,
            &self.sources,
            yandex.ok().and_then(Result::ok),
        ));
        let filter = evidence::exposure_filter(days);
        let exposure = tokio::time::timeout(
            OWNER_BUDGET,
            self.exposure.report_closed_with_health(&filter),
        )
        .await
        .ok()
        .and_then(Result::ok);
        observations.extend(evidence::exposure(days, &self.sources, exposure));
        self.journal.record_round(lease, &observations).await?;
        Ok(true)
    }
    pub async fn pages_once(&self, probe: Option<&PublicPageProbe>) -> Result<bool, SignalError> {
        let Some(lease) = self.journal.claim_round(0).await? else {
            return Ok(false);
        };
        let report = if let Some(probe) = probe {
            tokio::time::timeout(OWNER_BUDGET, self.pages.report(probe.origin()))
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        let origin = probe.map(PublicPageProbe::origin);
        self.journal
            .record_round(lease, &[providers::pages(origin, report)])
            .await?;
        Ok(true)
    }
}

pub fn start(accounts: Option<PgAccounts>) -> Vec<JoinHandle<()>> {
    let Some(accounts) = accounts else {
        return Vec::new();
    };
    let sources = SignalSources::from_environment();
    let probe = sources
        .origin
        .as_deref()
        .and_then(|origin| PublicPageProbe::new(origin).ok());
    let worker = SignalWorker::new(&accounts, sources);
    let signals = worker.clone();
    let aggregate = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut index = 0usize;
        loop {
            interval.tick().await;
            if signals.run_once([1, 7, 30, 90][index]).await.is_err() {
                tracing::warn!("FLOW aggregate signal round unavailable");
            }
            index = (index + 1) % 4;
            if index == 0 {
                let _ = signals.journal.prune().await;
            }
        }
    });
    let pages = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Some(probe) = &probe {
                let _ = probe.check_once(&worker.pages).await;
            }
            if worker.pages_once(probe.as_ref()).await.is_err() {
                tracing::warn!("FLOW public-page signal round unavailable");
            }
        }
    });
    vec![aggregate, pages]
}
