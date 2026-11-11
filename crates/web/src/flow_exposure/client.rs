use axum::{
    body::{to_bytes, Body, HttpBody},
    extract::Request,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
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
    let has_issuer = request
        .extensions()
        .get::<std::sync::Arc<dyn tor_contracts::exposure::ObservationIssuer>>()
        .is_some();
    let is_get = request.method() == Method::GET;
    // Axum executes GET handlers for HEAD and then discards HTML. Such requests
    // must not register opportunities for cards that were never delivered.
    if !is_get {
        request
            .extensions_mut()
            .remove::<std::sync::Arc<dyn tor_contracts::exposure::ObservationIssuer>>();
    }
    let mut response = next.run(request).await;
    let html_response = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("text/html"));
    // Cache isolation does not depend on successfully buffering or transforming
    // HTML. A renderer may have already issued opaque per-response proofs.
    if has_issuer && html_response {
        no_store(response.headers_mut());
    }
    if !is_get
        || response.status() != StatusCode::OK
        || !html_response
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
        Err(_) => {
            parts.status = StatusCode::SERVICE_UNAVAILABLE;
            parts.headers.remove(header::CONTENT_LENGTH);
            no_store(&mut parts.headers);
            return Response::from_parts(parts, Body::empty());
        }
    };
    let Ok(original) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if !original.contains("data-flow-proof=") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    no_store(&mut parts.headers);
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
        let updated = observation_policy(policy);
        if let Ok(value) = HeaderValue::from_str(&updated) {
            parts.headers.insert("content-security-policy", value);
        }
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(html))
}

fn no_store(headers: &mut HeaderMap) {
    headers.remove(header::ETAG);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn observation_policy(policy: &str) -> String {
    let mut has_connect = false;
    let mut directives = Vec::new();
    for directive in policy.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let mut tokens = directive.split_ascii_whitespace();
        let name = tokens.next().unwrap_or_default();
        has_connect |= name == "connect-src";
        if matches!(name, "script-src" | "connect-src")
            && tokens.next() == Some("'none'")
            && tokens.next().is_none()
        {
            directives.push(format!("{name} 'self'"));
        } else {
            directives.push(directive.to_owned());
        }
    }
    if !has_connect {
        directives.push("connect-src 'self'".into());
    }
    // Explicit separators also cover an existing CSP without a trailing ';'.
    format!("{};", directives.join("; "))
}

#[cfg(test)]
mod tests;
