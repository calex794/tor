use axum::{
    body::{to_bytes, Body, HttpBody},
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(super) async fn script() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/flow_observations.js"),
    )
        .into_response()
}
pub(super) async fn append_script(mut request: Request, next: Next) -> Response {
    // Axum executes GET handlers for HEAD and then discards HTML. Such requests
    // must not register opportunities for cards that were never delivered.
    if request.method() != axum::http::Method::GET {
        request
            .extensions_mut()
            .remove::<std::sync::Arc<dyn tor_contracts::exposure::ObservationIssuer>>();
    }
    let response = next.run(request).await;
    if response.status() != StatusCode::OK
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.starts_with("text/html"))
        || response
            .body()
            .size_hint()
            .exact()
            .is_none_or(|n| n > 2 * 1024 * 1024)
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Ok(original) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if !original.contains("data-flow-proof=") || original.contains("/assets/flow-observations.js") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let Some(at) = original.rfind("</body>") else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let mut html = original.to_owned();
    html.insert_str(
        at,
        "<script defer src=\"/assets/flow-observations.js\"></script>",
    );
    if let Some(policy) = parts
        .headers
        .get("content-security-policy")
        .and_then(|h| h.to_str().ok())
    {
        let updated = policy.replace("script-src 'none';", "script-src 'self';");
        let updated = if updated.contains("connect-src") {
            updated.replace("connect-src 'none';", "connect-src 'self';")
        } else {
            format!("{updated} connect-src 'self';")
        };
        if let Ok(value) = HeaderValue::from_str(&updated) {
            parts.headers.insert("content-security-policy", value);
        }
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    parts
        .headers
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Response::from_parts(parts, Body::from(html))
}
