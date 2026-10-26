//! Passive, one-pass Axum adapter for the independent RIL classifier.
//! No authorization, anti-bot policy, database writes or unbounded metrics here.
use axum::{
    body::Body,
    extract::{MatchedPath, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use std::sync::OnceLock;
use std::time::Instant;
use tor_route_intelligence::{classify, RouteObservation};
use tor_traffic_behavior::{
    repositories::PgRilRollups, services::PgRollupPump, BehaviorEvent, LatencyGroup, RouteCode,
    StatusGroup, TrafficRecorder,
};
use tor_traffic_behavior::VerifiedSubjectKey;

// Trusted-only response marker. Auth supplies it after a live DB session lookup
// and explicit opt-in; no client-controlled header is ever parsed as identity.
#[derive(Clone)]
pub(crate) struct VerifiedRilSubject(pub(crate) VerifiedSubjectKey);

// One bounded worker per process, shared across all router variants and clones.
// No pseudo-user is derived from an unverified cookie, IP, or query string.
static TRAFFIC: OnceLock<TrafficRecorder> = OnceLock::new();

/// Local privacy barrier. A separate cross-instance invalidation mechanism is
/// required before identified paths may be enabled on multiple web processes.
pub(crate) fn path_report_for_diagnostics() -> Option<tor_traffic_behavior::PathReport> {
    TRAFFIC.get()?.path_report_for_diagnostics()
}

pub(crate) async fn forget_subject(key: VerifiedSubjectKey) -> bool {
    let Some(recorder) = TRAFFIC.get().cloned() else {
        return true;
    };
    tokio::task::spawn_blocking(move || recorder.forget_subject_for_privacy(key))
        .await
        .ok()
        .flatten()
        == Some(true)
}

pub(crate) fn start_pg_background(repo: PgRilRollups) {
    let recorder = TRAFFIC
        .get_or_init(|| TrafficRecorder::start(10_000, 1_024))
        .clone();
    PgRollupPump::new(repo, recorder).spawn_background();
}

pub(crate) fn instrument(router: Router) -> Router {
    let recorder = TRAFFIC
        .get_or_init(|| TrafficRecorder::start(10_000, 1_024))
        .clone();
    instrument_with_recorder(router, recorder)
}

fn instrument_with_recorder(router: Router, recorder: TrafficRecorder) -> Router {
    // A known route returning 404 remains known; an unregistered URL is unknown.
    // Return the same 404 as Axum's default without echoing the URL.
    router
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn_with_state(recorder, observe))
}

async fn observe(
    State(recorder): State<TrafficRecorder>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // MatchedPath comes from Axum's route registry, NOT URI.path(), query or headers.
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    let passport = classify(template, request.method().as_str());
    request.extensions_mut().insert(passport);
    let started = Instant::now();
    let mut response = next.run(request).await;
    let elapsed = started.elapsed();
    let status = response.status().as_u16();
    let observation = RouteObservation::new(passport, status, elapsed);
    // Only the trusted authentication layer may set this internal response
    // extension after confirming a live session and explicit opt-in.
    let subject = if (200..300).contains(&status) && passport.route_known {
        response
            .extensions_mut()
            .remove::<VerifiedRilSubject>()
            .map(|marker| marker.0)
    } else {
        response.extensions_mut().remove::<VerifiedRilSubject>();
        None
    };
    // Anonymous counters remain independent from personal behavior paths.
    let _accepted = recorder.try_record(BehaviorEvent {
        route: RouteCode::from_family(passport.route_family),
        status: StatusGroup::from_status(status),
        latency: LatencyGroup::from_elapsed(elapsed),
        subject,
        at_millis: recorder.uptime_millis(),
    });
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
    async fn real_request_feeds_bounded_behavior_worker() {
        let recorder = TrafficRecorder::start(10, 16);
        let app = instrument_with_recorder(
            Router::new().route("/books/{id}", get(family)),
            recorder.clone(),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/books/102?password=PRIVATE")
                    .header("cookie", "session=UNVERIFIED-MUST-NOT-BECOME-SUBJECT")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let view = recorder.snapshot_for_diagnostics().expect("bounded worker");
        assert_eq!(view.total, 1);
        assert_eq!(view.count_route(RouteCode::Books), 1);
        assert_eq!(view.active_subjects, 0);
        assert_eq!(view.dropped_events, 0);
        let paths = recorder
            .path_report_for_diagnostics()
            .expect("offline path report");
        assert_eq!(paths.assessed_events, 0);
        assert_eq!(paths.active_subjects, 0);
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
