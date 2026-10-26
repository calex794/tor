//! Passive, one-pass Axum adapter for the independent RIL classifier.
//! No authorization, anti-bot policy, database writes or unbounded metrics here.
use axum::{
    body::Body,
    extract::MatchedPath,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use std::time::Instant;
use tor_route_intelligence::{classify, RouteObservation};

pub(crate) fn instrument(router: Router) -> Router {
    // A known route returning 404 remains known; an unregistered URL is unknown.
    // Return the same 404 as Axum's default without echoing the URL.
    router
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn(observe))
}

async fn observe(mut request: Request<Body>, next: Next) -> Response {
    // MatchedPath comes from Axum's route registry, NOT URI.path(), query or headers.
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    let passport = classify(template, request.method().as_str());
    request.extensions_mut().insert(passport);
    let started = Instant::now();
    let mut response = next.run(request).await;
    let observation =
        RouteObservation::new(passport, response.status().as_u16(), started.elapsed());
    // Future bounded metrics can inspect this value. Nothing is logged or persisted.
    response.extensions_mut().insert(observation);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, extract::Extension, http::Method, routing::get};
    use tor_route_intelligence::{CostClass, RoutePassport, StatusBucket};
    use tower::ServiceExt;

    async fn family(Extension(passport): Extension<RoutePassport>) -> &'static str {
        passport.route_family
    }

    #[tokio::test]
    async fn matched_path_injects_bounded_passport_and_observation() {
        let app = instrument(Router::new().route("/books/{id}", get(family)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/books/18446744073709551615?token=SECRET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let observed = *response
            .extensions()
            .get::<RouteObservation>()
            .expect("observation");
        assert!(observed.passport.route_known);
        assert_eq!(observed.passport.route_family, "/books/");
        assert_eq!(observed.status_bucket, StatusBucket::Success);
        assert!(response.headers().get("x-ril-passport").is_none());
        let text = to_bytes(response.into_body(), 256).await.unwrap();
        assert_eq!(text.as_ref(), b"/books/");
    }

    #[tokio::test]
    async fn unregistered_route_is_diagnostics_only_and_not_authorized_by_ril() {
        let app = instrument(Router::new().route("/new/{id}", get(family)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/new/42?secret=DO-NOT-LOG")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let observed = response
            .extensions()
            .get::<RouteObservation>()
            .expect("observation");
        assert!(!observed.passport.route_known);
        assert_eq!(observed.passport.route_family, "/unknown/");
        assert_eq!(response.status(), StatusCode::OK); // RIL must not become an ACL.
    }

    #[tokio::test]
    async fn method_overrides_are_observable_without_changing_http_methods() {
        let app = instrument(Router::new().route("/login", get(family).post(family)));
        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let post_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            get_response
                .extensions()
                .get::<RouteObservation>()
                .unwrap()
                .passport
                .cost_class,
            CostClass::CheapRead
        );
        assert_eq!(
            post_response
                .extensions()
                .get::<RouteObservation>()
                .unwrap()
                .passport
                .cost_class,
            CostClass::Write
        );
    }

    #[tokio::test]
    async fn unmatched_path_preserves_404_and_no_raw_url_labels() {
        let app = instrument(Router::new().route("/catalog", get(family)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/secret/path/ABC?signature=SECRET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let obs = response
            .extensions()
            .get::<RouteObservation>()
            .expect("fallback observation");
        assert_eq!(obs.passport.route_family, "/unknown/");
        assert_eq!(obs.status_bucket, StatusBucket::ClientError);
        let debug = format!("{obs:?}");
        assert!(!debug.contains("SECRET"));
        assert!(!debug.contains("ABC"));
    }
}
