//! Numeric causal evidence for a separately executed chapter-import attempt.
//! No job, book, account, object key, SQL text or client-provided identifier.
#[cfg(unix)]
pub mod lookup;

pub const FRAME_SIZE: usize = 80;
pub const MAX_DURATION_US: u64 = 86_400_000_000;
const MAGIC: &[u8; 4] = b"OBG1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Outcome {
    Completed = 0,
    Retried = 1,
    Failed = 2,
    Stale = 3,
    Interrupted = 4,
    Unavailable = 5,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Retried => "retried",
            Self::Failed => "failed",
            Self::Stale => "stale",
            Self::Interrupted => "interrupted",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundTrace {
    pub parent_trace_id: u128,
    pub trace_id: u128,
    pub attempt: u8,
    pub outcome: Outcome,
    /// 0: before source; 1: source; 2: parse; 3: commit; 4: cleanup.
    /// An interrupted attempt reports the last entered phase, not completion.
    pub phase: u8,
    pub duration_us: u64,
    /// Sequential wall times: verified source read, parse, commit, cleanup.
    /// These are not PostgreSQL execution time or CPU consumption.
    pub phase_us: [u64; 4],
}

impl BackgroundTrace {
    pub fn encode(&self) -> Option<[u8; FRAME_SIZE]> {
        let sum = self.phase_us.iter().try_fold(0_u64, |n, x| n.checked_add(*x))?;
        if self.parent_trace_id == 0
            || self.trace_id == 0
            || self.parent_trace_id == self.trace_id
            || !(1..=8).contains(&self.attempt)
            || self.phase > 4
            || self.duration_us > MAX_DURATION_US
            || sum > self.duration_us
            || (self.outcome == Outcome::Completed && self.phase != 4)
        {
            return None;
        }
        let mut data = [0_u8; FRAME_SIZE];
        data[..4].copy_from_slice(MAGIC);
        data[4..20].copy_from_slice(&self.parent_trace_id.to_be_bytes());
        data[20..36].copy_from_slice(&self.trace_id.to_be_bytes());
        data[36] = self.attempt;
        data[37] = self.outcome as u8;
        data[38] = self.phase;
        data[39] = 1; // Fixed kind: chapter import, never arbitrary text.
        data[40..48].copy_from_slice(&self.duration_us.to_be_bytes());
        for (i, us) in self.phase_us.iter().enumerate() {
            data[48 + i * 8..56 + i * 8].copy_from_slice(&us.to_be_bytes());
        }
        Some(data)
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() != FRAME_SIZE || !data.starts_with(MAGIC) || data[39] != 1 {
            return None;
        }
        let outcome = match data[37] {
            0 => Outcome::Completed,
            1 => Outcome::Retried,
            2 => Outcome::Failed,
            3 => Outcome::Stale,
            4 => Outcome::Interrupted,
            5 => Outcome::Unavailable,
            _ => return None,
        };
        let mut phase_us = [0_u64; 4];
        for (i, us) in phase_us.iter_mut().enumerate() {
            *us = u64::from_be_bytes(data[48 + i * 8..56 + i * 8].try_into().ok()?);
        }
        let trace = Self {
            parent_trace_id: u128::from_be_bytes(data[4..20].try_into().ok()?),
            trace_id: u128::from_be_bytes(data[20..36].try_into().ok()?),
            attempt: data[36],
            outcome,
            phase: data[38],
            duration_us: u64::from_be_bytes(data[40..48].try_into().ok()?),
            phase_us,
        };
        trace.encode()?;
        Some(trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BackgroundTrace {
        BackgroundTrace {
            parent_trace_id: 16,
            trace_id: 99,
            attempt: 2,
            outcome: Outcome::Completed,
            phase: 4,
            duration_us: 100_000,
            phase_us: [10_000, 20_000, 30_000, 5_000],
        }
    }

    #[test]
    fn numeric_background_frame_roundtrips_without_changing_http_wire() {
        let value = sample();
        let frame = value.encode().unwrap();
        assert_eq!(frame.len(), FRAME_SIZE);
        assert_eq!(BackgroundTrace::decode(&frame), Some(value));
        assert!(crate::services::wire::decode(&frame).is_err());
    }

    #[test]
    fn truncation_suffix_unknown_kind_and_outcome_are_rejected() {
        let frame = sample().encode().unwrap();
        for n in 0..FRAME_SIZE {
            assert!(BackgroundTrace::decode(&frame[..n]).is_none());
        }
        let mut suffix = frame.to_vec();
        suffix.extend_from_slice(b"private source text");
        assert!(BackgroundTrace::decode(&suffix).is_none());
        for index in [0, 36, 37, 38, 39] {
            let mut changed = frame;
            changed[index] = 255;
            assert!(BackgroundTrace::decode(&changed).is_none());
        }
    }

    #[test]
    fn invalid_identity_duration_and_phase_cannot_claim_completed_work() {
        let mut value = sample();
        value.trace_id = value.parent_trace_id;
        assert!(value.encode().is_none());
        value = sample();
        value.parent_trace_id = 0;
        assert!(value.encode().is_none());
        value = sample();
        value.phase = 3;
        assert!(value.encode().is_none());
        value = sample();
        value.duration_us = MAX_DURATION_US + 1;
        assert!(value.encode().is_none());
        value = sample();
        value.phase_us = [u64::MAX; 4];
        assert!(value.encode().is_none());
        value = sample();
        value.phase_us[0] = value.duration_us;
        assert!(value.encode().is_none());
    }

    #[test]
    fn interrupted_and_retry_records_are_not_success() {
        for outcome in [Outcome::Interrupted, Outcome::Retried, Outcome::Stale] {
            let mut value = sample();
            value.outcome = outcome;
            value.phase = 2;
            let frame = value.encode().unwrap();
            assert_eq!(BackgroundTrace::decode(&frame).unwrap().outcome, outcome);
            assert_ne!(outcome.label(), "completed");
        }
    }
}
