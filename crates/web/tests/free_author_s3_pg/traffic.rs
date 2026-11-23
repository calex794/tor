//! Inspect bodies sent by Axum without relying on Chromium retaining a
//! previous document's response after navigation.
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, StatusCode},
    middleware,
    response::Response,
    Router,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

#[derive(Default)]
pub struct Traffic {
    bytes: AtomicU64,
    responses: AtomicU64,
    no_script_sources: AtomicU64,
    leaked: AtomicBool,
    unreadable: AtomicBool,
}

impl Traffic {
    pub fn assert_clean(&self) {
        assert!(
            !self.unreadable.load(Ordering::SeqCst),
            "every reader response must be inspected"
        );
        assert!(
            !self.leaked.load(Ordering::SeqCst),
            "only the successful no-script source response may carry the chapter marker"
        );
        assert!(self.no_script_sources.load(Ordering::SeqCst) > 0);
        assert!(self.responses.load(Ordering::SeqCst) > 0);
        assert!(self.bytes.load(Ordering::SeqCst) > 0);
        println!(
            "S349_HTTP_EGRESS responses={} bytes={} no_script_sources={} chapter_source_leaked=false",
            self.responses.load(Ordering::SeqCst),
            self.bytes.load(Ordering::SeqCst),
            self.no_script_sources.load(Ordering::SeqCst),
        );
    }
}

pub fn instrument(app: Router, marker: &'static str) -> (Router, Arc<Traffic>) {
    let evidence = Arc::new(Traffic::default());
    let counter = evidence.clone();
    let app = app.layer(middleware::from_fn(
        move |request: axum::extract::Request, next: middleware::Next| {
            let counter = counter.clone();
            async move {
                let path = request.uri().path();
                // Axum strips the body of HEAD after this middleware returns.
                let inspect = request.method() != Method::HEAD
                    && (path.starts_with("/reader/")
                        || path.starts_with("/read/")
                        || path.starts_with("/me/library/saved/")
                        || path.contains("/admin-")
                        || path.ends_with("/free-grant"));
                let no_script_source = request.method() == Method::GET
                    && path
                        .strip_prefix("/api/v1/chapters/")
                        .and_then(|path| path.strip_suffix("/free-grant"))
                        .is_some_and(|chapter| chapter.parse::<i64>().is_ok_and(|id| id > 0))
                    && request.uri().query().is_some_and(|query| {
                        query
                            .split('&')
                            .any(|parameter| parameter == "format=source")
                    });
                let response = next.run(request).await;
                if !inspect {
                    return response;
                }
                let (parts, body) = response.into_parts();
                let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        counter.unreadable.store(true, Ordering::SeqCst);
                        return Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Body::empty())
                            .unwrap();
                    }
                };
                counter.responses.fetch_add(1, Ordering::SeqCst);
                counter
                    .bytes
                    .fetch_add(bytes.len() as u64, Ordering::SeqCst);
                if bytes
                    .windows(marker.len())
                    .any(|part| part == marker.as_bytes())
                {
                    if no_script_source
                        && parts.status == StatusCode::OK
                        && parts
                            .headers
                            .get(header::CONTENT_TYPE)
                            .is_some_and(|value| value == "text/plain; charset=utf-8")
                    {
                        counter.no_script_sources.fetch_add(1, Ordering::SeqCst);
                    } else {
                        counter.leaked.store(true, Ordering::SeqCst);
                    }
                }
                Response::from_parts(parts, Body::from(bytes))
            }
        },
    ));
    (app, evidence)
}
