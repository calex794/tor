//! Shared S034 HTTP helper for purchased encrypted chapter completion.
use super::*;

pub(super) async fn request_completion(
    app: &Router,
    method: &str,
    edition: (i64, i64, i64),
    session: &str,
    csrf: &str,
    body: Option<Value>,
) -> Response {
    let (chapter, purchase, version) = edition;
    let url = format!(
        "/api/v1/me/progress/purchased/complete?chapter_id={chapter}&purchase_id={purchase}&version_id={version}"
    );
    let mut request = Request::builder()
        .uri(if method == "GET" {
            url.as_str()
        } else {
            "/api/v1/me/progress/purchased/complete"
        })
        .method(method)
        .header(header::ORIGIN, ORIGIN)
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        )
        .header("x-csrf-token", csrf);
    if method == "POST" {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.map_or_else(String::new, |v| v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap()
}
