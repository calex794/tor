use super::*;
use MailPurpose::*;
use QuotaDecision::*;

fn state(attempts: i64) -> CostSnapshot {
    CostSnapshot {
        limits: CostLimits {
            enabled: true,
            warning_1: 10_000,
            warning_2: 30_000,
            news_pause: 40_000,
            monthly_limit: 50_000,
        },
        usage: Some(MonthUsage {
            month_start: "2026-11-01".to_owned(),
            free_allowance: 2000,
            baseline_kopecks: 0,
            rate_per_thousand: 8032,
            reserved_attempts: attempts,
        }),
        boundary_pause: false,
    }
}

#[test]
fn free_tier_is_not_a_registration_cutoff() {
    for (attempts, expected) in [(1999, 0), (2000, 0), (2001, 9), (5000, 24096)] {
        let value = state(attempts);
        assert_eq!(
            value.usage.as_ref().unwrap().estimate_kopecks(0),
            Some(expected)
        );
        assert_eq!(value.admission(RegistrationConfirmation), Reserved);
    }
}

#[test]
fn cost_envelope_reserves_headroom_for_transactional_mail() {
    let at_news_threshold = state(6979);
    assert_eq!(at_news_threshold.admission(ProjectNews), CostNewsPaused);
    assert_eq!(
        at_news_threshold.admission(RegistrationConfirmation),
        Reserved
    );
    assert_eq!(at_news_threshold.admission(SupportReply), Reserved);
    assert_eq!(state(8224).admission(SupportReply), Reserved);
    assert_eq!(
        state(8225).admission(RegistrationConfirmation),
        CostMonthlyLimit
    );
}

#[test]
fn unavailable_allowance_is_not_assumed_and_inconsistent_state_fails_closed() {
    let mut value = state(2000);
    let usage = value.usage.as_mut().unwrap();
    usage.free_allowance = 0;
    usage.baseline_kopecks = 10_000;
    assert_eq!(usage.estimate_kopecks(0), Some(26064));
    usage.rate_per_thousand = 8031;
    assert_eq!(value.admission(SupportReply), CostStateUnavailable);
    value.usage = None;
    assert_eq!(value.admission(SupportReply), CostStateUnavailable);
    value.limits.enabled = false;
    assert_eq!(value.admission(SupportReply), CostEmergencyStop);

    let mut boundary = state(0);
    boundary.boundary_pause = true;
    assert_eq!(boundary.admission(SupportReply), CostMonthBoundary);

    let mut overflow = state(i64::MAX);
    overflow.usage.as_mut().unwrap().rate_per_thousand = i64::MAX;
    assert_eq!(overflow.admission(SupportReply), CostStateUnavailable);
}

#[test]
fn previously_reserved_send_is_checked_again_without_double_charging() {
    let mut value = state(8225);
    assert_eq!(value.submission(SupportReply), Reserved);
    assert_eq!(value.admission(SupportReply), CostMonthlyLimit);
    assert_eq!(value.submission(ProjectNews), CostNewsPaused);
    value.limits.enabled = false;
    assert_eq!(value.submission(SupportReply), CostEmergencyStop);
}
