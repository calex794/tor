//! Shared HTTP test request helper to keep auth_postgres below the source cap.
use super::*;

pub(super) fn json_request(
    method: &str,
    uri: &str,
    body: &str,
    cookies: &str,
    origin: &str,
    csrf: &str,
) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))
        .expect("JSON HTTP");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40001))));
    req
}
