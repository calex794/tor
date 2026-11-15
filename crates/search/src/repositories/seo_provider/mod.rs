//! Read-only official provider clients. Fixed origins, no redirects and no raw error logging.
mod parse;
#[cfg(test)]
mod tests;
use crate::domain::seo_analytics::*;
use crate::services::seo_analytics::SearchAnalyticsProvider;
use async_trait::async_trait;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, RequestBuilder,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};
type E = SeoAnalyticsError;
const BODY_LIMIT: usize = 8 * 1024 * 1024;

pub struct ProviderClient {
    client: Client,
    source: SourceKey,
    base: String,
    yandex_user: Option<u64>,
}
impl ProviderClient {
    pub fn new(source: SourceKey, token: &str, yandex_user: Option<u64>) -> Result<Self, E> {
        source.validate()?;
        if token.len() < 8 || token.len() > 4096 || !token.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(E::Invalid);
        }
        if source.provider == Provider::Yandex
            && yandex_user.is_none_or(|v| v == 0 || v > i64::MAX as u64)
        {
            return Err(E::Invalid);
        }
        let prefix = if source.provider == Provider::Google {
            "Bearer"
        } else {
            "OAuth"
        };
        let mut auth =
            HeaderValue::from_str(&format!("{prefix} {token}")).map_err(|_| E::Invalid)?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .user_agent("TOR-Search-Aggregates/1")
            .build()
            .map_err(|_| E::Unavailable)?;
        let base = match source.provider {
            Provider::Google => "https://www.googleapis.com",
            Provider::Yandex => "https://api.webmaster.yandex.net",
        }
        .into();
        Ok(Self {
            client,
            source,
            base,
            yandex_user,
        })
    }
    async fn read(&self, request: RequestBuilder) -> Result<Value, E> {
        let mut response = request.send().await.map_err(|_| E::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 | 404 => return Err(E::Unauthorized),
            429 => return Err(E::RateLimited),
            _ => return Err(E::Unavailable),
        }
        if response
            .content_length()
            .is_some_and(|n| n > BODY_LIMIT as u64)
        {
            return Err(E::InvalidResponse);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| E::Unavailable)? {
            if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return Err(E::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| E::InvalidResponse)
    }
    async fn google(&self, day: &str) -> Result<Snapshot, E> {
        let url = format!(
            "{}/webmasters/v3/sites/{}/searchAnalytics/query",
            self.base,
            segment(&self.source.property)
        );
        let totals_response=self.read(self.client.post(&url).json(&json!({"startDate":day,"endDate":day,"type":"web","aggregationType":"byProperty","dataState":"final","rowLimit":1}))).await?;
        if totals_response
            .get("responseAggregationType")
            .and_then(Value::as_str)
            .is_some_and(|v| v != "byProperty")
        {
            return Err(E::InvalidResponse);
        }
        let total_rows = parse::google_rows(&totals_response)?;
        if total_rows.len() > 1 {
            return Err(E::InvalidResponse);
        }
        let totals = total_rows.first().map(parse::google_metrics).transpose()?;
        let mut pages = BTreeMap::new();
        let mut provider_rows = 0;
        let mut omitted_rows = 0;
        let mut limit_reached = false;
        for offset in [0, 25_000] {
            let response=self.read(self.client.post(&url).json(&json!({"startDate":day,"endDate":day,"type":"web","aggregationType":"auto","dimensions":["page"],"dataState":"final","rowLimit":25000,"startRow":offset}))).await?;
            if response
                .get("responseAggregationType")
                .and_then(Value::as_str)
                .is_some_and(|v| v != "byPage")
            {
                return Err(E::InvalidResponse);
            }
            let rows = parse::google_rows(&response)?;
            if rows.len() > 25_000 {
                return Err(E::InvalidResponse);
            }
            provider_rows += rows.len() as u64;
            for row in rows {
                let keys = row["keys"].as_array().ok_or(E::InvalidResponse)?;
                if keys.len() != 1 {
                    return Err(E::InvalidResponse);
                }
                let url = keys[0].as_str().ok_or(E::InvalidResponse)?;
                if !parse::insert(
                    &mut pages,
                    &self.source.origin,
                    url,
                    parse::google_metrics(row)?,
                )? {
                    omitted_rows += 1;
                }
            }
            if rows.len() < 25_000 {
                break;
            }
            if offset == 25_000 {
                limit_reached = true;
            }
        }
        Ok(Snapshot {
            totals,
            pages: pages
                .into_iter()
                .map(|(path, metrics)| PageMetrics { path, metrics })
                .collect(),
            provider_rows,
            omitted_rows,
            limit_reached,
        })
    }
    async fn yandex(&self, day: &str) -> Result<Snapshot, E> {
        let base = format!(
            "{}/v4/user/{}/hosts/{}",
            self.base,
            self.yandex_user.ok_or(E::Invalid)?,
            segment(&self.source.property)
        );
        // The wide UTC interval includes this provider-labelled date in every
        // timezone. Preserve its returned date, rather than assuming it is UTC.
        let from = format!("{day}T00:00:00.000+14:00");
        let to = format!("{day}T23:59:59.999-12:00");
        let totals_response = self
            .read(
                self.client
                    .get(format!("{base}/search-queries/all/history"))
                    .query(&[
                        ("query_indicator", "TOTAL_CLICKS"),
                        ("query_indicator", "TOTAL_SHOWS"),
                        ("query_indicator", "AVG_SHOW_POSITION"),
                        ("device_type_indicator", "ALL"),
                        ("date_from", from.as_str()),
                        ("date_to", to.as_str()),
                    ]),
            )
            .await?;
        let totals = parse::yandex_totals(&totals_response, day)?;
        let mut pages = BTreeMap::new();
        let mut provider_rows = 0;
        let mut omitted_rows = 0;
        let mut limit_reached = false;
        let mut expected = None;
        for offset in (0..MAX_PAGES).step_by(500) {
            let response=self.read(self.client.post(format!("{base}/query-analytics/list")).json(&json!({
                "offset":offset,"limit":500,"device_type_indicator":"ALL","search_location":"WEB_LOCATION","text_indicator":"URL",
                "sort_by_date":{"date":day,"statistic_field":"IMPRESSIONS","by":"DESC"}
            }))).await?;
            let count = parse::count(&response["count"])?;
            if expected.replace(count).is_some_and(|old| old != count) {
                return Err(E::InvalidResponse);
            }
            let rows = response["text_indicator_to_statistics"]
                .as_array()
                .ok_or(E::InvalidResponse)?;
            if rows.len() > 500 {
                return Err(E::InvalidResponse);
            }
            provider_rows += rows.len() as u64;
            for row in rows {
                if row["text_indicator"]["type"].as_str() != Some("URL") {
                    return Err(E::InvalidResponse);
                }
                let url = row["text_indicator"]["value"]
                    .as_str()
                    .ok_or(E::InvalidResponse)?;
                if let Some(metrics) = parse::yandex_metrics(row, day)? {
                    if !parse::insert(&mut pages, &self.source.origin, url, metrics)? {
                        omitted_rows += 1;
                    }
                } else {
                    omitted_rows += 1;
                }
            }
            if rows.len() < 500 || provider_rows >= count {
                limit_reached = provider_rows < count;
                break;
            }
            if offset + 500 >= MAX_PAGES {
                limit_reached = true;
            }
        }
        Ok(Snapshot {
            totals,
            pages: pages
                .into_iter()
                .map(|(path, metrics)| PageMetrics { path, metrics })
                .collect(),
            provider_rows,
            omitted_rows,
            limit_reached,
        })
    }
}
#[async_trait]
impl SearchAnalyticsProvider for ProviderClient {
    async fn collect(&self, day: &str) -> Result<Snapshot, E> {
        if !valid_day(day) {
            return Err(E::Invalid);
        }
        let snapshot = match self.source.provider {
            Provider::Google => self.google(day).await?,
            Provider::Yandex => self.yandex(day).await?,
        };
        snapshot.validate(&self.source.origin)?;
        Ok(snapshot)
    }
}
fn segment(value: &str) -> String {
    let mut s = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            s.push(byte as char)
        } else {
            use std::fmt::Write;
            write!(&mut s, "%{byte:02X}").expect("String");
        }
    }
    s
}
