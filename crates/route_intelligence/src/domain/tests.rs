use super::*;
use std::collections::HashSet;

#[test]
fn registry_is_finite_and_classifies_each_declared_method() {
    let mut unique = HashSet::new();
    for registered in registry() {
        assert!(unique.insert((registered.template, registered.method)));
        let p = classify(Some(registered.template), registered.method);
        assert!(p.route_known, "{:?}", registered);
        assert_eq!(p.route_family, registered.family);
        assert_eq!(p.route_match_kind, registered.match_kind);
        assert_eq!(p.traffic_surface, registered.surface);
        assert_eq!(p.product_area, registered.area);
        assert_eq!(p.cost_class, registered.cost);
        assert_eq!(p.risk_hint, registered.risk);
        assert_eq!(p.ril_taxonomy_version, RIL_TAXONOMY_VERSION);
        assert_eq!(p.ril_classifier_version, RIL_CLASSIFIER_VERSION);
        assert_eq!(
            p.route_known_definition_version,
            ROUTE_KNOWN_DEFINITION_VERSION
        );
    }
    assert_eq!(unique.len(), 24);
}

#[test]
fn public_private_and_write_routes_are_distinct() {
    let get = classify(Some("/login"), "GET");
    let post = classify(Some("/login"), "POST");
    assert_eq!(get.product_area, ProductArea::Auth);
    assert_eq!(get.cost_class, CostClass::CheapRead);
    assert_eq!(post.cost_class, CostClass::Write);
    assert_eq!(
        classify(Some("/read/{id}/place"), "POST").cost_class,
        CostClass::Write
    );
    assert_eq!(
        classify(Some("/me/library"), "GET").product_area,
        ProductArea::Reading
    );
    assert_eq!(
        classify(Some("/me/books"), "GET").product_area,
        ProductArea::Dashboard
    );
    assert_eq!(
        classify(Some("/me/books"), "POST").cost_class,
        CostClass::Write
    );
    assert_eq!(classify(Some("/me/books/{id}"), "GET").route_family, "/me/");
    assert_eq!(
        classify(Some("/me/books/{id}/chapters/new"), "GET").product_area,
        ProductArea::Dashboard
    );
    assert_eq!(
        classify(Some("/me/books/{id}/chapters"), "POST").cost_class,
        CostClass::Write
    );
    assert_eq!(
        classify(Some("/me/books/{id}/publish"), "POST").cost_class,
        CostClass::Write
    );
    assert_eq!(
        classify(Some("/api/v1/books"), "GET").traffic_surface,
        TrafficSurface::Api
    );
    assert_eq!(
        classify(Some("/catalog"), "HEAD").cost_class,
        CostClass::CheapRead
    );
}

#[test]
fn unknown_and_unregistered_method_never_get_normal_labels() {
    for (route, method) in [
        (Some("/admin/secret/{id}"), "GET"),
        (Some("/read/{id}"), "POST"),
        (Some("/login"), "DELETE"),
        (None, "GET"),
        (Some("/books/123456"), "GET"), // raw paths cannot masquerade as MatchedPath
    ] {
        let p = classify(route, method);
        assert!(!p.route_known);
        assert_eq!(p.route_family, "/unknown/");
        assert_eq!(p.route_decision, RouteDecision::Diagnostics);
        assert_eq!(p.route_match_kind, RouteMatchKind::Unknown);
        assert_ne!(p.risk_hint, RiskHint::Normal);
    }
}

#[test]
fn one_hundred_thousand_ids_do_not_create_new_label_series() {
    // Axum reports this STATIC template for every /books/<id>.
    let unique_families: HashSet<_> = (0..100_000_u64)
        .map(|_| {
            let p = classify(Some("/books/{id}"), "GET");
            assert_eq!(p.route_match_kind, RouteMatchKind::Prefix);
            (
                p.route_family,
                p.product_area.as_str(),
                p.cost_class.as_str(),
            )
        })
        .collect();
    assert_eq!(unique_families.len(), 1);
    assert_eq!(unique_families.into_iter().next().unwrap().0, "/books/");
}

#[test]
fn bounded_latency_and_status_buckets() {
    let p = classify(Some("/api/v1/books"), "GET");
    assert_eq!(
        RouteObservation::new(p, 200, Duration::from_millis(1)).status_bucket,
        StatusBucket::Success
    );
    assert_eq!(
        RouteObservation::new(p, 404, Duration::from_millis(20)).latency_bucket,
        LatencyBucket::Under100Ms
    );
    assert_eq!(
        RouteObservation::new(p, 503, Duration::from_secs(3)).latency_bucket,
        LatencyBucket::Slow
    );
    assert_eq!(
        RouteObservation::new(p, 503, Duration::from_secs(3)).status_bucket,
        StatusBucket::ServerError
    );
}

#[test]
fn passports_cannot_embed_query_or_user_values() {
    let p = classify(Some("/books/{id}"), "GET");
    let s = format!("{p:?}");
    assert!(!s.contains("access_token"));
    assert!(!s.contains("presigned"));
    assert!(!s.contains("book_slug"));
    assert_eq!(p.route_family, "/books/");
}
