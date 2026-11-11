use super::*;

#[test]
fn adequate_registered_opportunities_with_low_ack_are_a_quality_alert() {
    let mut e = complete(Some(0), Some(100));
    e.current_opportunities = 100;
    e.previous_opportunities = 100;
    e.current_sample = 0;
    e.current_ack_per_mille = Some(0);
    assert_eq!(
        evaluate(Rule::ExposureQuality, e).unwrap().status,
        Status::Active
    );
    assert_eq!(
        evaluate(Rule::ExposureChange, e).unwrap().status,
        Status::Unknown
    );
    e.current_opportunities = 19;
    assert_eq!(
        evaluate(Rule::ExposureQuality, e).unwrap().status,
        Status::Unknown
    );
}

fn complete(current: Option<i64>, previous: Option<i64>) -> Evidence {
    Evidence {
        available: true,
        current,
        previous,
        current_units: 24,
        previous_units: 24,
        expected_units: 24,
        current_sample: 20,
        previous_sample: 20,
        current_ack_per_mille: Some(800),
        previous_ack_per_mille: Some(800),
        ..Evidence::missing(true)
    }
}
#[test]
fn missing_and_partial_counts_never_mean_zero_or_recovery() {
    let e = complete(None, Some(100));
    assert_eq!(
        evaluate(Rule::HtmlChange, e).unwrap().status,
        Status::Unknown
    );
    let mut e = complete(Some(0), Some(100));
    e.current_units = 23;
    assert_eq!(
        evaluate(Rule::HtmlChange, e).unwrap().status,
        Status::Unknown
    );
    assert_eq!(
        evaluate(Rule::HtmlQuality, e).unwrap().status,
        Status::Active
    );
}
#[test]
fn decrease_boundary_and_large_counts_are_exact() {
    assert_eq!(
        evaluate(Rule::HtmlChange, complete(Some(70), Some(100)))
            .unwrap()
            .status,
        Status::Active
    );
    assert_eq!(
        evaluate(Rule::HtmlChange, complete(Some(71), Some(100)))
            .unwrap()
            .status,
        Status::Healthy
    );
    assert_eq!(
        evaluate(Rule::ReadingCompletions, complete(Some(35), Some(50)))
            .unwrap()
            .status,
        Status::Active
    );
    assert_eq!(
        evaluate(Rule::ReadingCompletions, complete(Some(0), Some(49)))
            .unwrap()
            .status,
        Status::Unknown
    );
    assert_eq!(
        evaluate(Rule::HtmlChange, complete(Some(i64::MAX), Some(i64::MAX)))
            .unwrap()
            .status,
        Status::Healthy
    );
}
#[test]
fn uncertain_observer_and_partial_unknown_entities_do_not_close_episode() {
    let mut e = complete(Some(0), Some(100));
    e.current_ack_per_mille = Some(799);
    assert_eq!(
        evaluate(Rule::ExposureChange, e).unwrap().status,
        Status::Unknown
    );
    e.unknown_entities = 1;
    assert_eq!(
        evaluate(Rule::UnderexposedBooks, e).unwrap().status,
        Status::Unknown
    );
    e.current = Some(1);
    assert_eq!(
        evaluate(Rule::UnderexposedGenres, e).unwrap().status,
        Status::Active
    );
    e.known_discards = 1;
    assert_eq!(
        evaluate(Rule::UnderexposedGenres, e).unwrap().status,
        Status::Unknown
    );
}
#[test]
fn active_reason_change_and_unknown_preserve_the_same_episode() {
    let active = Evaluation::new(Status::Active, Reason::SourceFailure);
    let first = transition(Episode::default(), active, 10);
    assert_eq!(first.event, Some(EventKind::Opened));
    let unknown = transition(
        first.episode,
        Evaluation::new(Status::Unknown, Reason::PartialCoverage),
        20,
    );
    assert_eq!(unknown.episode, first.episode);
    let changed = transition(
        unknown.episode,
        Evaluation::new(Status::Active, Reason::PartialCoverage),
        30,
    );
    assert_eq!(changed.event, None);
    assert_eq!(changed.episode.sequence, 1);
    assert_eq!(
        transition(
            changed.episode,
            Evaluation::new(Status::Healthy, Reason::WithinThreshold),
            40
        )
        .event,
        Some(EventKind::Recovered)
    );
}
#[test]
fn suppressed_opening_has_no_orphan_recovery_and_active_remains_active() {
    let active = Evaluation::new(Status::Active, Reason::RecordedDrop);
    let healthy = Evaluation::new(Status::Healthy, Reason::WithinThreshold);
    let first = transition(Episode::default(), active, 1);
    let recovered = transition(first.episode, healthy, 2);
    let suppressed = transition(recovered.episode, active, 3);
    assert_eq!(suppressed.event, None);
    assert_eq!(suppressed.episode.reason, Some(Reason::RecordedDrop));
    assert_eq!(transition(suppressed.episode, healthy, 4).event, None);
    assert_eq!(
        transition(recovered.episode, active, COOLDOWN_MS + 1).event,
        Some(EventKind::Opened)
    );
}
#[test]
fn source_scope_is_unambiguous_and_disabled_preserves_episode() {
    assert_ne!(source_scope(&["ab", "c"]), source_scope(&["a", "bc"]));
    assert!(valid_scope(&source_scope(&["configured-origin", "v2"])));
    let first = transition(
        Episode::default(),
        Evaluation::new(Status::Active, Reason::LowExposure),
        1,
    );
    let disabled = evaluate(Rule::UnderexposedBooks, Evidence::missing(false)).unwrap();
    assert_eq!(disabled.status, Status::Disabled);
    assert_eq!(
        transition(first.episode, disabled, 2).episode,
        first.episode
    );
}
