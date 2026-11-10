//! FLOW visit measurements belong to F, outside anonymous RIL telemetry.
//! An account activity window is not a browser identity or a unique person.
use rand::{rngs::OsRng, RngCore};

pub const FLOW_VISIT_IDLE_SECONDS: i64 = 1_800;
pub const FLOW_VISIT_MAX_DELAY_SECONDS: i64 = 300;
pub const FLOW_VISIT_FUTURE_SECONDS: i64 = 5;
pub const FLOW_VISIT_DAYS: [i32; 4] = [1, 7, 30, 90];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowScope {
    pub subject_id: String,
    pub epoch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisitObservation {
    pub event_id: String,
    pub scope: FlowScope,
    pub observed_at_millis: i64,
    /// 0 home, 1 catalogue, 2 book, 3 authorized reader, 4 library.
    pub surface: i16,
    /// Untrusted request hint: 0 search, 1 referral, 3 direct/unknown.
    /// Code 2 is reserved and cannot be claimed from an HTTP referrer.
    pub acquisition_hint: i16,
}

impl VisitObservation {
    pub fn new(
        scope: FlowScope,
        observed_at_millis: i64,
        surface: i16,
        acquisition_hint: i16,
    ) -> Result<Self, FlowVisitError> {
        let mut random = [0_u8; 16];
        OsRng
            .try_fill_bytes(&mut random)
            .map_err(|_| FlowVisitError::Unavailable)?;
        random[6] = (random[6] & 0x0f) | 0x40;
        random[8] = (random[8] & 0x3f) | 0x80;
        let hex: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let observation = Self {
            event_id: format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            ),
            scope,
            observed_at_millis,
            surface,
            acquisition_hint,
        };
        observation.validate()?;
        Ok(observation)
    }

    pub fn validate(&self) -> Result<(), FlowVisitError> {
        if !valid_uuid(&self.event_id)
            || !valid_uuid(&self.scope.subject_id)
            || !valid_uuid(&self.scope.epoch)
            || self.observed_at_millis <= 0
            || !(0..=4).contains(&self.surface)
            || !matches!(self.acquisition_hint, 0 | 1 | 3)
        {
            return Err(FlowVisitError::Invalid);
        }
        Ok(())
    }
}

pub(crate) fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
        && value
            .bytes()
            .any(|byte| byte.is_ascii_hexdigit() && byte != b'0')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowVisitError {
    Invalid,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowVisitsRow {
    pub acquisition_hint: i16,
    pub landing_surface: i16,
    pub last_surface: i16,
    pub visits: i64,
    pub pageviews: i64,
    /// Difference between first and last observed page, not reading time.
    pub observed_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowVisitsPeriod {
    pub from_utc: String,
    pub until_utc: String,
    /// Visits are assigned to the day of their first observed page.
    pub visits: Option<i64>,
    pub pageviews: Option<i64>,
    /// Distinct currently retained consent subjects with any observed page in
    /// the full period. Never a sum of daily distincts; unknown beyond 30 days.
    pub unique_subjects: Option<i64>,
    pub observed_seconds: Option<i64>,
    /// Pages continuing an exceptionally long visit whose old origin was
    /// erased. They cannot be attributed to a retained visit-start cohort.
    pub unattributed_pageviews: Option<i64>,
    pub rows: Vec<FlowVisitsRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowVisitsReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current: FlowVisitsPeriod,
    pub previous: FlowVisitsPeriod,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> FlowScope {
        FlowScope {
            subject_id: "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".into(),
            epoch: "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb".into(),
        }
    }

    #[test]
    fn observation_has_a_stable_server_receipt_and_finite_payload() {
        let first = VisitObservation::new(scope(), 1_800_000_000_000, 3, 0).unwrap();
        assert!(valid_uuid(&first.event_id));
        assert_eq!(first.event_id.as_bytes()[14], b'4');
        assert_eq!(first.clone().event_id, first.event_id);
        assert_ne!(
            first.event_id,
            VisitObservation::new(scope(), 1_800_000_000_000, 3, 0)
                .unwrap()
                .event_id
        );
        for (surface, hint) in [(-1, 0), (5, 0), (0, -1), (0, 2), (0, 4)] {
            assert_eq!(
                VisitObservation::new(scope(), 1, surface, hint),
                Err(FlowVisitError::Invalid)
            );
        }
        for invalid in ["", "' OR true --", "00000000-0000-0000-0000-000000000000"] {
            assert!(!valid_uuid(invalid));
        }
    }
}
