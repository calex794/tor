//! F: bounded, unpredictable discovery surprises. No click-triggered rerolls.
//! This module deliberately knows nothing about wallets, reading progress or HTTP.
use sha2::{Digest, Sha256};

/// The probabilities below are pilot defaults, not a promise of a prize.
pub const DEFAULT_PROBABILITY_PER_MILLE: u16 = 350;
pub const MIN_ACTIVE_SECONDS: u32 = 120;
pub const MAX_ACTIVE_SECONDS: u32 = 1200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurpriseSchedule {
    pub enabled: bool,
    pub probability_per_mille: u16,
}

impl Default for SurpriseSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            probability_per_mille: DEFAULT_PROBABILITY_PER_MILLE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Disabled,
    NotSelected,
    Planned(Plan),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plan {
    /// Counts only visible and active time; background tabs do not advance it.
    pub active_after_seconds: u32,
    /// Opaque server-side tickets, never coordinates or public odds.
    pub anchor_ticket: u64,
    pub content_ticket: u64,
}

/// An HMAC-SHA256 implementation with a fixed 32-byte server-provided key.
/// Domain labels make appearance, timing, placement and content independent.
fn hmac_draw(key: &[u8; 32], subject: &[u8; 32], period: i64, label: &[u8]) -> u64 {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(b"TOR-surprise-v1");
    inner.update(label);
    inner.update(subject);
    inner.update(period.to_be_bytes());
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    let result = outer.finalize();
    u64::from_be_bytes(result[..8].try_into().expect("SHA256 has eight bytes"))
}

fn draw_range(value: u64, start: u32, end: u32) -> u32 {
    start + (value % u64::from(end - start + 1)) as u32
}

/// The caller owns the stable opaque session subject and period; changing a
/// URL, clicking, refreshing or opening another tab must not change either.
/// Limits and eligibility must be enforced atomically in the repository.
pub fn decide(
    key: &[u8; 32],
    subject: &[u8; 32],
    period: i64,
    config: SurpriseSchedule,
) -> Result<Decision, &'static str> {
    if config.probability_per_mille > 1000 {
        return Err("invalid surprise probability");
    }
    if !config.enabled {
        return Ok(Decision::Disabled);
    }
    let chance = hmac_draw(key, subject, period, b"appearance") % 1000;
    if chance >= u64::from(config.probability_per_mille) {
        return Ok(Decision::NotSelected);
    }

    // Uneven time windows: 20% at 2-5m, 50% at 5-11m, 30% at 11-20m.
    // A second independent draw chooses the second inside the selected window.
    let bucket = hmac_draw(key, subject, period, b"window") % 1000;
    let (start, end) = if bucket < 200 {
        (120, 299)
    } else if bucket < 700 {
        (300, 659)
    } else {
        (660, 1200)
    };
    let when = draw_range(
        hmac_draw(key, subject, period, b"window-second"),
        start,
        end,
    );
    Ok(Decision::Planned(Plan {
        active_after_seconds: when,
        anchor_ticket: hmac_draw(key, subject, period, b"anchor"),
        content_ticket: hmac_draw(key, subject, period, b"content"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [0x93; 32];
    const SESSION: [u8; 32] = [0x27; 32];

    #[test]
    fn default_off_and_invalid_policy_fail_closed() {
        assert_eq!(
            decide(&KEY, &SESSION, 10, SurpriseSchedule::default()),
            Ok(Decision::Disabled)
        );
        let invalid = SurpriseSchedule {
            enabled: true,
            probability_per_mille: 1001,
        };
        assert!(decide(&KEY, &SESSION, 10, invalid).is_err());
        let none = SurpriseSchedule {
            enabled: true,
            probability_per_mille: 0,
        };
        assert_eq!(decide(&KEY, &SESSION, 10, none), Ok(Decision::NotSelected));
    }

    #[test]
    fn same_session_period_has_exactly_one_decision_without_click_inputs() {
        let cfg = SurpriseSchedule {
            enabled: true,
            probability_per_mille: 1000,
        };
        let original = decide(&KEY, &SESSION, 123, cfg).unwrap();
        for _ in 0..100 {
            assert_eq!(decide(&KEY, &SESSION, 123, cfg), Ok(original));
        }
        let Decision::Planned(plan) = original else {
            panic!("100% selection must produce a plan");
        };
        assert!((MIN_ACTIVE_SECONDS..=MAX_ACTIVE_SECONDS).contains(&plan.active_after_seconds));
        assert_ne!(plan.anchor_ticket, plan.content_ticket);
    }

    #[test]
    fn independently_keyed_periods_and_sessions_produce_different_plans() {
        let cfg = SurpriseSchedule {
            enabled: true,
            probability_per_mille: 1000,
        };
        let mut another_session = SESSION;
        another_session[31] ^= 1;
        let mut another_key = KEY;
        another_key[1] ^= 1;
        let base = decide(&KEY, &SESSION, 100, cfg);
        assert_ne!(base, decide(&KEY, &SESSION, 101, cfg));
        assert_ne!(base, decide(&KEY, &another_session, 100, cfg));
        assert_ne!(base, decide(&another_key, &SESSION, 100, cfg));
    }

    #[test]
    fn varying_periods_have_bounded_selection_rate_and_timing() {
        let cfg = SurpriseSchedule {
            enabled: true,
            probability_per_mille: DEFAULT_PROBABILITY_PER_MILLE,
        };
        let count = (0..10_000)
            .filter(|period| {
                matches!(
                    decide(&KEY, &SESSION, *period, cfg),
                    Ok(Decision::Planned(_))
                )
            })
            .count();
        assert!(
            (3200..3800).contains(&count),
            "unexpected pilot frequency: {count}"
        );
    }
}
