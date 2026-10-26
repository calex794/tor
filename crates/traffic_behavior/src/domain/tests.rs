use super::*;

fn key(value: u128) -> VerifiedSubjectKey {
    VerifiedSubjectKey::from_hmac_digest(value.to_be_bytes())
}
fn event(route: RouteCode, subject: Option<VerifiedSubjectKey>, at_millis: u64) -> BehaviorEvent {
    BehaviorEvent {
        route,
        status: StatusGroup::Success,
        latency: LatencyGroup::Under10Ms,
        subject,
        at_millis,
    }
}

#[test]
fn route_codes_are_finite_even_for_untrusted_strings() {
    let mut families = std::collections::HashSet::new();
    for n in 0..100_000 {
        let input = format!("/books/{n}?token=SECRET");
        families.insert(RouteCode::from_family(&input));
    }
    assert_eq!(families, std::collections::HashSet::from([RouteCode::Unknown]));
    assert_eq!(RouteCode::from_family("/books/"), RouteCode::Books);
    assert_eq!(RouteCode::from_family("/read/"), RouteCode::Read);
    assert_eq!(RouteCode::from_family("/api/v1/books/"), RouteCode::ApiBooks);
}

#[test]
fn anonymous_events_do_not_link_independent_visitors() {
    let mut sketch = BehaviorSketch::new(MAX_SUBJECTS);
    for n in 0..100_000 {
        sketch.record(event(RouteCode::Books, None, n));
    }
    let s = sketch.snapshot();
    assert_eq!(s.total, 100_000);
    assert_eq!(s.count_route(RouteCode::Books), 100_000);
    assert_eq!(s.active_subjects, 0);
    assert_eq!(s.count_transition(RouteCode::Books, RouteCode::Books, GapBucket::Immediate), 0);
}

#[test]
fn verified_subject_transitions_are_bounded_and_explainable() {
    let subject = key(17);
    let mut sketch = BehaviorSketch::new(10);
    sketch.record(event(RouteCode::Catalog, Some(subject), 0));
    sketch.record(event(RouteCode::Books, Some(subject), 100));
    sketch.record(event(RouteCode::Read, Some(subject), 2_100));
    sketch.record(event(RouteCode::Catalog, Some(subject), 9_100));
    let s = sketch.snapshot();
    assert_eq!(s.active_subjects, 1);
    assert_eq!(s.total, 4);
    assert_eq!(s.count_transition(RouteCode::Catalog, RouteCode::Books, GapBucket::Immediate), 1);
    assert_eq!(s.count_transition(RouteCode::Books, RouteCode::Read, GapBucket::Fast), 1);
    assert_eq!(s.count_transition(RouteCode::Read, RouteCode::Catalog, GapBucket::Normal), 1);
    assert_eq!(s.count_route(RouteCode::Catalog), 2);
}

#[test]
fn eviction_has_one_index_entry_per_subject_and_no_stale_transition() {
    let mut sketch = BehaviorSketch::new(2);
    sketch.record(event(RouteCode::Home, Some(key(1)), 1));
    sketch.record(event(RouteCode::Catalog, Some(key(2)), 2));
    sketch.record(event(RouteCode::Books, Some(key(1)), 3)); // refresh first subject
    sketch.record(event(RouteCode::Reader, Some(key(3)), 4)); // evict key 2
    sketch.record(event(RouteCode::Read, Some(key(2)), 5)); // new subject, not a transition
    let s = sketch.snapshot();
    assert_eq!(s.active_subjects, 2);
    assert_eq!(s.evicted_subjects, 2);
    assert_eq!(s.count_transition(RouteCode::Home, RouteCode::Books, GapBucket::Immediate), 1);
    assert_eq!(s.count_transition(RouteCode::Catalog, RouteCode::Read, GapBucket::Immediate), 0);
}

#[test]
fn ten_thousand_subjects_respect_hard_cap() {
    let mut sketch = BehaviorSketch::new(usize::MAX);
    for i in 0..10_001u128 {
        sketch.record(event(RouteCode::Books, Some(key(i)), i as u64));
    }
    let s = sketch.snapshot();
    assert_eq!(s.active_subjects, MAX_SUBJECTS);
    assert_eq!(s.total, 10_001);
    assert_eq!(s.evicted_subjects, 1);
}

#[test]
fn idle_pseudonyms_expire_and_opt_out_removes_subject() {
    let mut sketch = BehaviorSketch::new(3);
    sketch.record(event(RouteCode::Books, Some(key(1)), 0));
    sketch.record(event(RouteCode::Catalog, None, SUBJECT_TTL_MS + 1));
    assert_eq!(sketch.snapshot().active_subjects, 0);
    assert_eq!(sketch.snapshot().evicted_subjects, 1);
    sketch.record(event(RouteCode::Reader, Some(key(1)), SUBJECT_TTL_MS + 2));
    assert_eq!(sketch.snapshot().active_subjects, 1);
    assert!(sketch.forget_subject(key(1)));
    assert!(!sketch.forget_subject(key(1)));
    assert_eq!(sketch.snapshot().active_subjects, 0);
}

#[test]
fn duration_and_status_have_only_fixed_buckets() {
    assert_eq!(StatusGroup::from_status(404), StatusGroup::ClientError);
    assert_eq!(StatusGroup::from_status(503), StatusGroup::ServerError);
    assert_eq!(LatencyGroup::from_elapsed(Duration::from_millis(9)), LatencyGroup::Under10Ms);
    assert_eq!(LatencyGroup::from_elapsed(Duration::from_secs(4)), LatencyGroup::Slow);
    assert_eq!(GapBucket::from_millis(3_000_000), GapBucket::Long);
}
