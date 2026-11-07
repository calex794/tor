//! FLOW-02: first-party request classification into finite audience buckets.
//! Headers are inspected transiently, never stored as raw referrer, UA or URI.
use axum::http::{header, HeaderMap, Method, Uri};
use tor_contracts::flow::{
    AcquisitionChannel, PageViewEvidence, RequestMethod, ResponseKind, RouteSurface, TrafficHygiene,
};
use tor_traffic_behavior::FlowPageview;

pub(super) struct FlowRequest {
    surface: RouteSurface,
    method: RequestMethod,
    suspected_automation: bool,
    acquisition_hint: AcquisitionChannel,
}

impl FlowRequest {
    pub(super) fn new(template: Option<&str>, method: &Method, headers: &HeaderMap) -> Self {
        let surface = match template {
            Some("/") => RouteSurface::Home,
            Some("/catalog") => RouteSurface::Catalogue,
            Some("/books/{id}") => RouteSurface::BookDetail,
            Some("/reader/{id}") | Some("/read/{id}") => RouteSurface::ChapterReader,
            Some("/me")
            | Some("/me/library")
            | Some("/me/books")
            | Some("/me/books/new")
            | Some("/me/books/{id}") => RouteSurface::AuthenticatedPage,
            _ => RouteSurface::Unknown,
        };
        Self {
            surface,
            method: if method == Method::GET {
                RequestMethod::Get
            } else {
                RequestMethod::Other
            },
            suspected_automation: likely_bot(headers),
            acquisition_hint: referrer_hint(headers),
        }
    }

    /// The handler must have returned an actual successful HTML response from
    /// an authorized known route. Unclassified visitors count as requests only.
    pub(super) fn observed(
        &self,
        status: u16,
        headers: &HeaderMap,
        route_known: bool,
        opted_in: bool,
    ) -> Option<FlowPageview> {
        let html = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let value = value.trim().to_ascii_lowercase();
                value == "text/html" || value.starts_with("text/html;")
            });
        let evidence = PageViewEvidence {
            method: self.method,
            status,
            response_kind: if html {
                ResponseKind::Html
            } else {
                ResponseKind::Other
            },
            surface: self.surface,
            hygiene: if self.suspected_automation {
                TrafficHygiene::SuspectedAutomation
            } else if opted_in {
                TrafficHygiene::Eligible
            } else {
                TrafficHygiene::Unclassified
            },
            visibility_confirmed: route_known,
        };
        evidence.observed_html_request().then_some(FlowPageview {
            surface: self.surface,
            acquisition_hint: self.acquisition_hint,
        })
    }
}

/// Low-cost heuristic for known automation. Absence of this flag is NOT
/// proof of a human reader, and must never influence ACL or charging.
fn likely_bot(headers: &HeaderMap) -> bool {
    let Some(ua) = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 256)
    else {
        return false;
    };
    let ua = ua.to_ascii_lowercase();
    [
        "bot",
        "crawler",
        "spider",
        "curl/",
        "wget/",
        "python-requests",
        "headlesschrome",
    ]
    .iter()
    .any(|token| ua.contains(token))
}

/// Untrusted request-level referrer *hint*, not a known first-touch source.
/// Internal navigation must not be mislabeled an external referral.
fn referrer_hint(headers: &HeaderMap) -> AcquisitionChannel {
    let Some(host) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 512)
        .and_then(|value| value.parse::<Uri>().ok())
        .and_then(|uri| uri.host().map(|host| host.to_ascii_lowercase()))
    else {
        return AcquisitionChannel::DirectOrUnknown;
    };
    let host = host.trim_end_matches('.');
    let current = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .unwrap_or_default();
    if host.eq_ignore_ascii_case(current) {
        return AcquisitionChannel::DirectOrUnknown;
    }
    if [
        "google.com",
        "google.nl",
        "google.de",
        "google.co.uk",
        "google.ru",
        "yandex.ru",
        "yandex.com",
        "bing.com",
        "duckduckgo.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        AcquisitionChannel::ExternalOrganicSearch
    } else {
        AcquisitionChannel::ExternalReferral
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn html() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        h
    }

    #[test]
    fn flow_02_known_html_pages_are_requests_not_anonymous_humans() {
        let request = FlowRequest::new(Some("/books/{id}"), &Method::GET, &HeaderMap::new());
        assert!(request.observed(200, &html(), true, false).is_some());
        assert!(request.observed(200, &html(), true, true).is_some());
        assert!(request.observed(404, &html(), true, true).is_none());
        assert!(request.observed(200, &html(), false, true).is_none());
        assert!(request.observed(301, &html(), true, true).is_none());
        assert!(
            FlowRequest::new(Some("/api/v1/books"), &Method::GET, &HeaderMap::new())
                .observed(200, &html(), true, true)
                .is_none()
        );
        assert!(
            FlowRequest::new(Some("/books/{id}"), &Method::POST, &HeaderMap::new())
                .observed(200, &html(), true, true)
                .is_none()
        );
        let mut non_html = HeaderMap::new();
        non_html.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(request.observed(200, &non_html, true, true).is_none());
    }

    #[test]
    fn flow_02_suspected_bots_are_excluded_without_persisting_ua() {
        let mut request = HeaderMap::new();
        request.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Googlebot/2.1"),
        );
        assert!(FlowRequest::new(Some("/catalog"), &Method::GET, &request)
            .observed(200, &html(), true, false)
            .is_none());
        assert!(!likely_bot(&HeaderMap::new()));
    }

    #[test]
    fn flow_02_referrer_is_only_a_coarse_hint_and_internal_is_not_external() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.org"));
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://www.google.com/search?q=PRIVATE"),
        );
        assert_eq!(
            referrer_hint(&headers),
            AcquisitionChannel::ExternalOrganicSearch
        );
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://example.org/books/123?secret=PRIVATE"),
        );
        assert_eq!(referrer_hint(&headers), AcquisitionChannel::DirectOrUnknown);
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://www.google.com.attacker.net/search"),
        );
        assert_eq!(
            referrer_hint(&headers),
            AcquisitionChannel::ExternalReferral
        );
        assert_eq!(
            referrer_hint(&HeaderMap::new()),
            AcquisitionChannel::DirectOrUnknown
        );
    }
}
