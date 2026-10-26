use super::*;

fn key(i: u128) -> VerifiedSubjectKey {
    VerifiedSubjectKey::from_hmac_digest(i.to_be_bytes())
}

fn event(
    route: RouteCode,
    status: StatusGroup,
    subject: Option<VerifiedSubjectKey>,
    at: u64,
) -> BehaviorEvent {
    BehaviorEvent {
        route,
        status,
        latency: super::super::LatencyGroup::Under10Ms,
        subject,
        at_millis: at,
    }
}
fn ok(route: RouteCode, id: u128, at: u64) -> BehaviorEvent {
    event(route, StatusGroup::Success, Some(key(id)), at)
}

#[test]
fn actual_scanner_like_sequence_is_diagnostic_reject_not_acl() {
    let mut paths = PathAnalyzer::new(40);
    let mut last = None;
    for i in 0..24 {
        last = paths.record(event(
            RouteCode::Unknown,
            StatusGroup::ClientError,
            Some(key(22)),
            200 + i * 100,
        ));
    }
    let a = last.unwrap();
    assert_eq!(a.version, PATH_VERSION);
    assert_eq!(a.events, 24);
    assert_eq!(a.hygiene, Hygiene::Reject);
    assert!(a.reasons.unknown_probes && a.reasons.rapid_unknowns);
    assert_eq!(paths.report().reject, 9); // first reject at event 16
}

#[test]
fn ordinary_reader_navigation_and_long_pause_never_trigger_scanner() {
    let mut paths = PathAnalyzer::new(4);
    for i in 0..32 {
        let route = if i % 2 == 0 {
            RouteCode::Reader
        } else {
            RouteCode::Read
        };
        let a = paths.record(ok(route, 17, i * 900)).unwrap();
        assert_eq!(a.hygiene, Hygiene::Normal);
        assert!(!a.reasons.unknown_probes);
    }
    // The earlier session expires. A long break is NOT a scanner indicator.
    let a = paths
        .record(ok(RouteCode::Read, 17, SUBJECT_TTL_MS + 50_000))
        .unwrap();
    assert_eq!(a.hygiene, Hygiene::Normal);
    assert_eq!(a.events, 1);
    assert_eq!(paths.report().evicted_subjects, 1);
}

#[test]
fn aba_backtracking_and_errors_surface_explainable_friction() {
    let mut paths = PathAnalyzer::new(5);
    let mut last = None;
    for i in 0..24 {
        let route = if i % 2 == 0 {
            RouteCode::Catalog
        } else {
            RouteCode::Books
        };
        last = paths.record(event(
            route,
            StatusGroup::ClientError,
            Some(key(2)),
            i * 200,
        ));
    }
    let a = last.unwrap();
    assert!(a.backtracks >= 5);
    assert!(a.reasons.backtracking);
    assert_eq!(a.hygiene, Hygiene::Degraded);
    assert_eq!(a.unknown, 0);
}

#[test]
fn anonymous_traffic_never_creates_shared_paths_or_identity() {
    let mut paths = PathAnalyzer::new(MAX_SUBJECTS);
    for i in 0..100_000 {
        assert!(paths
            .record(event(RouteCode::Unknown, StatusGroup::ClientError, None, i))
            .is_none());
    }
    assert_eq!(paths.report(), PathReport::default());
}

#[test]
fn independent_pseudonyms_are_isolated_even_when_interleaved() {
    let mut paths = PathAnalyzer::new(2);
    for i in 0..15 {
        let a = paths
            .record(event(
                RouteCode::Unknown,
                StatusGroup::ClientError,
                Some(key(1)),
                i * 100,
            ))
            .unwrap();
        let b = paths.record(ok(RouteCode::Reader, 2, i * 100)).unwrap();
        assert_eq!(b.hygiene, Hygiene::Normal);
        assert_eq!(b.unknown, 0);
        assert!(a.unknown > 0);
    }
    assert_eq!(paths.report().active_subjects, 2);
}

#[test]
fn ten_thousand_subjects_and_32_points_per_subject_bound_memory() {
    let mut paths = PathAnalyzer::new(usize::MAX);
    for i in 0..10_001_u128 {
        paths.record(ok(RouteCode::Catalog, i, i as u64));
    }
    assert_eq!(paths.report().active_subjects, MAX_SUBJECTS);
    assert_eq!(paths.report().evicted_subjects, 1);
    for i in 0..1000_u64 {
        let a = paths
            .record(ok(RouteCode::Books, 9_999, 10_100 + i))
            .unwrap();
        assert!(usize::from(a.events) <= PATH_WINDOW);
    }
}

#[test]
fn forgotten_subject_starts_a_new_sequence_without_leaking_key() {
    let mut paths = PathAnalyzer::new(8);
    for i in 0..20 {
        paths.record(event(
            RouteCode::Unknown,
            StatusGroup::ClientError,
            Some(key(51)),
            i,
        ));
    }
    assert!(paths.forget_subject(key(51)));
    assert!(!paths.forget_subject(key(51)));
    let fresh = paths.record(ok(RouteCode::Catalog, 51, 100)).unwrap();
    assert_eq!(fresh.events, 1);
    assert_eq!(fresh.hygiene, Hygiene::Normal);
    assert_eq!(paths.report().active_subjects, 1);
    let diagnostic = format!("{fresh:?}");
    assert!(!diagnostic.contains("51"));
    assert!(!diagnostic.contains("user_id"));
}

#[test]
fn out_of_order_events_and_idle_expiry_do_not_create_negative_gaps() {
    let mut paths = PathAnalyzer::new(2);
    assert_eq!(
        paths
            .record(ok(RouteCode::Catalog, 4, 1_000))
            .unwrap()
            .events,
        1
    );
    assert_eq!(
        paths.record(ok(RouteCode::Books, 4, 900)).unwrap().events,
        2
    );
    paths.record(ok(RouteCode::Catalog, 5, 1_000 + SUBJECT_TTL_MS + 1));
    assert_eq!(paths.report().active_subjects, 1);
    assert_eq!(paths.report().evicted_subjects, 1);
    assert_eq!(
        paths
            .record(ok(RouteCode::Books, 4, 1_000 + SUBJECT_TTL_MS + 2))
            .unwrap()
            .events,
        1
    );
}

#[test]
fn fixed_window_discards_old_unknown_probes_after_clean_behavior() {
    let mut paths = PathAnalyzer::new(3);
    for i in 0..20 {
        paths.record(event(
            RouteCode::Unknown,
            StatusGroup::ClientError,
            Some(key(5)),
            i * 100,
        ));
    }
    let mut a = paths.record(ok(RouteCode::Catalog, 5, 2_100)).unwrap();
    for i in 0..40 {
        a = paths
            .record(ok(RouteCode::Read, 5, 2_200 + i * 500))
            .unwrap();
    }
    assert_eq!(a.events as usize, PATH_WINDOW);
    assert_eq!(a.hygiene, Hygiene::Normal);
    assert_eq!(a.unknown, 0);
    assert_eq!(a.errors, 0);
}

#[test]
fn periodic_expiry_does_not_require_new_tracking_events() {
    let mut paths = PathAnalyzer::new(10);
    paths.record(ok(RouteCode::Books, 5, 2));
    paths.prune_idle(SUBJECT_TTL_MS + 3);
    assert_eq!(paths.report().active_subjects, 0);
    assert_eq!(paths.report().evicted_subjects, 1);
}
