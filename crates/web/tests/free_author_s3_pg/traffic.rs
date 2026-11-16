//! Inspect the exact HTTP bytes handed to Axum's socket, without relying on
//! Chromium retaining a previous document's response body after navigation.
use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
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
            "TOR must not carry the chapter source marker"
        );
        assert!(self.responses.load(Ordering::SeqCst) > 0);
        assert!(self.bytes.load(Ordering::SeqCst) > 0);
        println!(
            "S349_HTTP_EGRESS responses={} bytes={} chapter_source_leaked=false",
            self.responses.load(Ordering::SeqCst),
            self.bytes.load(Ordering::SeqCst)
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
                let inspect = path.starts_with("/reader/")
                    || path.starts_with("/read/")
                    || path.starts_with("/me/library/saved/")
                    || path.contains("/admin-")
                    || path.ends_with("/free-grant");
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
                    counter.leaked.store(true, Ordering::SeqCst);
                }
                Response::from_parts(parts, Body::from(bytes))
            }
        },
    ));
    (app, evidence)
}
