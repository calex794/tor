use super::*;

#[test]
fn repeated_signature_headers_are_not_accepted() {
    let mut headers = HeaderMap::new();
    assert_eq!(single_header(&headers, "x-tor-mail-signature"), None);
    headers.append("x-tor-mail-signature", "first".parse().unwrap());
    assert_eq!(
        single_header(&headers, "x-tor-mail-signature"),
        Some("first")
    );
    headers.append("x-tor-mail-signature", "second".parse().unwrap());
    assert_eq!(single_header(&headers, "x-tor-mail-signature"), None);
}

#[test]
fn no_store_on_all_relay_handler_outcomes() {
    for status in [
        StatusCode::NO_CONTENT,
        StatusCode::UNAUTHORIZED,
        StatusCode::CONFLICT,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let reply = response(status);
        assert_eq!(reply.status(), status);
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    }
}
