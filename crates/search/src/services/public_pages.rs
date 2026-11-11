//! Real HEAD requests to the configured public origin, without reader credentials.
use crate::{domain::public_pages::*, repositories::public_pages::PgPublicPages};
use reqwest::{Client, Url};
use std::{net::IpAddr, time::Duration};

#[derive(Clone)]
pub struct PublicPageProbe {
    client: Client,
    origin: String,
}
impl PublicPageProbe {
    pub fn new(raw: &str) -> Result<Self, PageProbeError> {
        let url = Url::parse(raw).map_err(|_| PageProbeError::Invalid)?;
        let loopback = url.host_str().is_some_and(|host| host=="localhost"
            || host.trim_matches(['[',']']).parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()));
        if url.scheme()!="https" && !(url.scheme()=="http" && loopback)
            || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some()
            || url.path()!="/" || url.query().is_some() || url.fragment().is_some() {
            return Err(PageProbeError::Invalid);
        }
        let origin = url.origin().ascii_serialization();
        if origin.len()>255 { return Err(PageProbeError::Invalid); }
        let client = Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .user_agent("TOR-AvailabilityBot/1.0")
            .connect_timeout(Duration::from_millis(300)).timeout(Duration::from_millis(750))
            .build().map_err(|_| PageProbeError::Unavailable)?;
        Ok(Self { client, origin })
    }
    pub fn origin(&self) -> &str { &self.origin }
    pub async fn check_once(&self, store: &PgPublicPages) -> Result<bool, PageProbeError> {
        tokio::time::timeout(Duration::from_secs(5), self.check(store)).await
            .map_err(|_| PageProbeError::Unavailable)?
    }
    async fn check(&self, store: &PgPublicPages) -> Result<bool, PageProbeError> {
        let Some(lease) = store.claim(&self.origin).await? else { return Ok(false); };
        let mut results = Vec::with_capacity(4);
        // Exactly four fixed slots; no inventory scan or unbounded request fanout.
        for pair in lease.targets.chunks(2) {
            let (first, second) = tokio::join!(self.check_target(store, &pair[0]), self.check_target(store, &pair[1]));
            results.push(first?);
            results.push(second?);
        }
        store.save(&lease, &results).await?;
        Ok(true)
    }
    async fn check_target(&self, store: &PgPublicPages, target: &PageTarget) -> Result<PageProbeResult, PageProbeError> {
        let status = if let Some(path) = target.path() {
            if store.current(target).await? {
                let response = self.client.head(format!("{}{path}", self.origin)).header("accept", "text/html")
                    .send().await.ok().map(|response| response.status().as_u16());
                if store.current(target).await? { response } else { None }
            } else { None }
        } else { None };
        Ok(PageProbeResult { target: target.clone(), status })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_configured_credential_free_origins_are_accepted() {
        for good in ["https://books.example", "http://127.0.0.1:8080", "http://localhost:8080", "http://[::1]:8080"] {
            assert!(PublicPageProbe::new(good).is_ok(), "{good}");
        }
        for bad in ["http://books.example", "https://user:secret@books.example", "https://books.example/book", "https://books.example/?token=x", "https://books.example/#x", "file:///etc/passwd"] {
            assert!(PublicPageProbe::new(bad).is_err(), "{bad}");
        }
    }
}
