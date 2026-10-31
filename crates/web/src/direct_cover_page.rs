//! Opt-in direct S3 cover HTML bridge. The input is server-rendered markup:
//! only canonical numeric /covers/{id}/{variant} src attributes are changed.
//! No page should embed presigned URLs, S3 keys or object bytes.
use axum::{
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Response},
};

const COVER_SRC: &str = "src=\"/covers/";
const SCRIPT: &str = "<script type=\"module\" src=\"/assets/newboo-direct-cover.mjs\"></script>\n</body>";

pub(super) fn decorate(html: Html<String>, origin: Option<&str>) -> (Html<String>, bool) {
    if origin.is_none() || !html.0.contains("</body>") {
        return (html, false);
    }
    let mut result = String::with_capacity(html.0.len() + 200);
    let mut rest = html.0.as_str();
    let mut found = false;

    while let Some(at) = rest.find(COVER_SRC) {
        result.push_str(&rest[..at]);
        let value = &rest[at + COVER_SRC.len()..];
        let digits = value.bytes().take_while(|byte| byte.is_ascii_digit()).count();
        let id = &value[..digits];
        let tail = &value[digits..];
        let variant = if tail.starts_with("/card.webp\"") {
            Some(("card.webp", "/card.webp\"".len()))
        } else if tail.starts_with("/main.webp\"") {
            Some(("main.webp", "/main.webp\"".len()))
        } else {
            None
        };
        if !id.is_empty() && !id.starts_with('0') && variant.is_some() {
            let (kind, consumed) = variant.expect("checked variant");
            result.push_str(&format!(
                "data-direct-cover=\"/api/v1/covers/{id}/{kind}/direct-grant\""
            ));
            rest = &tail[consumed..];
            found = true;
        } else {
            result.push_str(COVER_SRC);
            rest = value;
        }
    }
    result.push_str(rest);
    if !found {
        return (html, false);
    }
    (Html(result.replacen("</body>", SCRIPT, 1)), true)
}

pub(super) fn apply_policy(
    mut response: Response,
    direct_origin: Option<&str>,
    contains_direct_covers: bool,
) -> Response {
    if !contains_direct_covers {
        return response;
    }
    let Some(origin) = direct_origin else {
        return response;
    };
    // The operator-configured origin has already been checked at S3 startup;
    // only this exact HTTPS origin (or loopback in CI) may receive fetches.
    let policy = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; \
         connect-src 'self' {origin}; font-src 'self'; form-action 'self'; base-uri 'none'; \
         object-src 'none'; frame-ancestors 'none'"
    );
    let Ok(policy) = HeaderValue::from_str(&policy) else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub(super) fn page(html: Html<String>, origin: Option<&str>) -> Response {
    let (html, enabled) = decorate(html, origin);
    apply_policy(html.into_response(), origin, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://storage.example";

    #[test]
    fn disabled_pages_keep_their_current_same_origin_images() {
        let html = Html(
            "<html><body><img src=\"/covers/42/card.webp\"></body></html>".to_owned(),
        );
        let (html, enabled) = decorate(html, None);
        assert!(!enabled);
        assert!(html.0.contains("src=\"/covers/42/card.webp\""));
        assert!(!html.0.contains("direct-grant"));
    }

    #[test]
    fn enabled_public_pages_do_not_request_proxy_bytes() {
        let html = Html(
            "<html><body><img src=\"/covers/42/card.webp\" alt=\"\">\
             <img src=\"/covers/42/main.webp\" alt=\"\"></body></html>"
                .to_owned(),
        );
        let (html, enabled) = decorate(html, Some(ORIGIN));
        assert!(enabled);
        assert_eq!(html.0.matches("data-direct-cover=").count(), 2);
        assert!(!html.0.contains("src=\"/covers/"));
        assert!(html.0.contains("/api/v1/covers/42/card.webp/direct-grant"));
        assert!(html.0.contains("/api/v1/covers/42/main.webp/direct-grant"));
        assert_eq!(html.0.matches("newboo-direct-cover.mjs").count(), 1);
        let response = apply_policy(Html(html.0).into_response(), Some(ORIGIN), enabled);
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("connect-src 'self' https://storage.example;"));
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("img-src 'self' blob:"));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "private, no-store");
    }

    #[test]
    fn unknown_or_malicious_urls_are_never_rewritten() {
        for url in [
            "/covers/0/card.webp",
            "/covers/02/card.webp",
            "/covers/42/source.webp",
            "/covers/42/../card.webp",
            "/covers/x/card.webp",
            "/covers/42/main.webp?fake",
        ] {
            let html = Html(format!("<body><img src=\"{url}\"></body>"));
            let (out, enabled) = decorate(html, Some(ORIGIN));
            assert!(!enabled, "{url}");
            assert!(out.0.contains(&format!("src=\"{url}\"")));
        }
    }

    #[test]
    fn non_cover_pages_do_not_receive_relaxed_security_policy() {
        let response = page(
            Html("<html><body>Без обложек</body></html>".to_owned()),
            Some(ORIGIN),
        );
        assert!(!response.headers().contains_key(header::CONTENT_SECURITY_POLICY));
    }
}
