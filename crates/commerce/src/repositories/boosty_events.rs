//! Strictly read-only Boosty owner event feed; the origin is fixed.
//! No token refresh, provider writes, PostgreSQL or wallet credit path.
use crate::services::boosty_events::{
    valid_blog, valid_cursor, ObserveError, MAX_PAGE_ROWS, MAX_RESPONSE_BYTES,
};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use std::time::Duration;

pub struct BoostyEventFeed {
    client: reqwest::Client,
}

impl BoostyEventFeed {
    pub fn new(token: &str) -> Result<Self, ObserveError> {
        if token.is_empty() || token.len() > 4096 {
            return Err(ObserveError::InvalidConfig);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ObserveError::InvalidConfig)?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("TOR-Boosty-Observer/0.1"));
        headers.insert("x-app", HeaderValue::from_static("web"));
        headers.insert("x-currency", HeaderValue::from_static("RUB"));
        headers.insert("x-locale", HeaderValue::from_static("ru_RU"));
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| ObserveError::InvalidConfig)?;
        Ok(Self { client })
    }

    pub async fn fetch(&self, blog: &str, limit: usize, offset: Option<&str>)
        -> Result<Vec<u8>, ObserveError>
    {
        if !valid_blog(blog) || !(1..=MAX_PAGE_ROWS).contains(&limit)
            || offset.is_some_and(|s| !valid_cursor(s))
        {
            return Err(ObserveError::InvalidConfig);
        }
        let url = format!("https://api.boosty.to/v1/blog/stat/{blog}/events");
        let mut request = self.client.get(url).query(&[
            ("event_types", "subscribe,recurrent".to_owned()),
            ("limit", limit.to_string()),
        ]);
        if let Some(cursor) = offset {
            request = request.query(&[("offset", cursor)]);
        }
        let mut response = request.send().await.map_err(|_| ObserveError::Network)?;
        if !response.status().is_success() {
            return Err(ObserveError::HttpStatus(response.status().as_u16()));
        }
        if response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
            return Err(ObserveError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ObserveError::Network)? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Err(ObserveError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}
