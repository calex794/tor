//! E / S018: bounded UTC cadence policy based on confirmed first chapter starts.
//! No transport selection, contact consent, queue, or external delivery.
use chrono::{DateTime, Duration, Timelike, Utc};
use super::notification_preferences::DeliveryMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HourConfidence {
    None,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferredSendHour {
    pub hour_utc: u8,
    pub confidence: HourConfidence,
    pub observed_starts: u32,
}

impl PreferredSendHour {
    pub const fn is_reliable(self) -> bool {
        matches!(self.confidence, HourConfidence::Medium | HourConfidence::High)
    }
}

/// NEwBOO's 24 bounded UTC-hour buckets: ties prefer the earliest UTC hour.
/// Only an established pattern (medium/high confidence) justifies delaying.
pub fn choose_preferred_hour(counts: &[u32; 24]) -> PreferredSendHour {
    let mut hour = 12_u8;
    let mut top = 0_u32;
    let mut second = 0_u32;
    let mut total = 0_u32;
    for (candidate, raw) in counts.iter().copied().enumerate() {
        let count = raw.min(1_000);
        total = total.saturating_add(count);
        if count > top {
            second = top;
            top = count;
            hour = candidate as u8;
        } else if count > second {
            second = count;
        }
    }
    if total == 0 {
        return PreferredSendHour {
            hour_utc: 12,
            confidence: HourConfidence::None,
            observed_starts: 0,
        };
    }
    let confidence = if total >= 10 && top >= second.saturating_mul(2).max(1) {
        HourConfidence::High
    } else if total >= 3 && top >= 2 {
        HourConfidence::Medium
    } else {
        HourConfidence::Low
    };
    PreferredSendHour {
        hour_utc: hour,
        confidence,
        observed_starts: total,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendTiming {
    Disabled,
    Now,
    AtUtc(DateTime<Utc>),
}

fn next_utc_hour(now: DateTime<Utc>, hour: u8) -> DateTime<Utc> {
    let mut next = now
        .with_hour(u32::from(hour))
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .expect("validated 0..23 UTC hour");
    if next <= now {
        next += Duration::days(1);
    }
    next
}

/// Pure, deterministic decision for a future verified transport. This does
/// not authorize sending or make an unread notification into a grant.
pub fn plan_send_time(
    mode: DeliveryMode,
    preference: PreferredSendHour,
    now: DateTime<Utc>,
) -> SendTiming {
    match mode {
        DeliveryMode::Disabled => SendTiming::Disabled,
        DeliveryMode::Immediate => SendTiming::Now,
        DeliveryMode::DailyDigest => SendTiming::AtUtc(next_utc_hour(now, 12)),
        DeliveryMode::SmartTime if preference.is_reliable() => {
            SendTiming::AtUtc(next_utc_hour(now, preference.hour_utc))
        }
        DeliveryMode::SmartTime => SendTiming::Now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_hour_follows_bounded_evidence_not_clock_guesswork() {
        let none = choose_preferred_hour(&[0; 24]);
        assert_eq!(none.hour_utc, 12);
        assert_eq!(none.confidence, HourConfidence::None);
        let mut low = [0; 24];
        low[5] = 1;
        assert_eq!(choose_preferred_hour(&low).confidence, HourConfidence::Low);
        let mut tie = [0; 24];
        tie[4] = 2;
        tie[8] = 2;
        let medium = choose_preferred_hour(&tie);
        assert_eq!(medium.hour_utc, 4);
        assert_eq!(medium.confidence, HourConfidence::Medium);
        let mut strong = [0; 24];
        strong[21] = 9;
        strong[8] = 3;
        let high = choose_preferred_hour(&strong);
        assert_eq!(high.hour_utc, 21);
        assert_eq!(high.confidence, HourConfidence::High);
        strong[21] = u32::MAX;
        assert_eq!(choose_preferred_hour(&strong).observed_starts, 1_003);
    }

    #[test]
    fn four_modes_and_exact_utc_hour_boundary_are_deterministic() {
        let now = DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut counts = [0; 24];
        counts[16] = 5;
        let preferred = choose_preferred_hour(&counts);
        assert_eq!(
            plan_send_time(DeliveryMode::Disabled, preferred, now),
            SendTiming::Disabled
        );
        assert_eq!(
            plan_send_time(DeliveryMode::Immediate, preferred, now),
            SendTiming::Now
        );
        assert_eq!(
            plan_send_time(DeliveryMode::SmartTime, preferred, now),
            SendTiming::AtUtc(
                DateTime::parse_from_rfc3339("2026-11-10T16:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert_eq!(
            plan_send_time(DeliveryMode::DailyDigest, preferred, now),
            SendTiming::AtUtc(
                DateTime::parse_from_rfc3339("2026-11-11T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert_eq!(
            plan_send_time(DeliveryMode::SmartTime, choose_preferred_hour(&[0; 24]), now),
            SendTiming::Now
        );
    }
}
