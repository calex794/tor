//! MAIL-01h1: conservative local admission estimate in integer kopecks.
use super::quota::QuotaDecision;
use tor_contracts::mail::MailPurpose;

/// Rounded UP from 80.31666666 RUB/1000 (Russia, VAT included).
/// All paid attempts are charged at this upper tier, even above 10,000.
pub const MIN_RATE_KOPECKS_PER_THOUSAND: i64 = 8032;
pub const MAX_FREE_ALLOWANCE: i64 = 2000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostLimits {
    pub enabled: bool,
    pub warning_1: i64,
    pub warning_2: i64,
    pub news_pause: i64,
    pub monthly_limit: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonthUsage {
    pub month_start: String,
    /// Verified free units available exclusively to this admission gate.
    pub free_allowance: i64,
    /// Conservative prior/external charge allowance, not a billing receipt.
    pub baseline_kopecks: i64,
    pub rate_per_thousand: i64,
    pub reserved_attempts: i64,
}

impl MonthUsage {
    pub fn estimate_kopecks(&self, additional: i64) -> Option<i64> {
        if !(0..=MAX_FREE_ALLOWANCE).contains(&self.free_allowance)
            || self.baseline_kopecks < 0
            || self.rate_per_thousand < MIN_RATE_KOPECKS_PER_THOUSAND
            || self.reserved_attempts < 0
            || additional < 0
        {
            return None;
        }
        let total = i128::from(self.reserved_attempts.checked_add(additional)?);
        let paid = (total - i128::from(self.free_allowance)).max(0);
        let numerator = paid.checked_mul(i128::from(self.rate_per_thousand))?;
        let rounded = numerator.checked_add(999)? / 1000;
        i64::try_from(rounded.checked_add(i128::from(self.baseline_kopecks))?).ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CostSnapshot {
    pub limits: CostLimits,
    pub usage: Option<MonthUsage>,
    pub boundary_pause: bool,
}

impl CostSnapshot {
    pub fn admission(&self, purpose: MailPurpose) -> QuotaDecision {
        self.evaluate(purpose, 1)
    }

    /// A previously reserved attempt already counts towards the total.
    /// Re-evaluate current policy without charging it a second time.
    pub fn submission(&self, purpose: MailPurpose) -> QuotaDecision {
        self.evaluate(purpose, 0)
    }

    fn evaluate(&self, purpose: MailPurpose, additional: i64) -> QuotaDecision {
        use QuotaDecision::*;
        let l = self.limits;
        if !l.enabled {
            return CostEmergencyStop;
        }
        if !(0 < l.warning_1
            && l.warning_1 <= l.warning_2
            && l.warning_2 <= l.news_pause
            && l.news_pause < l.monthly_limit)
        {
            return CostStateUnavailable;
        }
        let Some(next) = self
            .usage
            .as_ref()
            .and_then(|u| u.estimate_kopecks(additional))
        else {
            return CostStateUnavailable;
        };
        if self.boundary_pause {
            return CostMonthBoundary;
        }
        if next > l.monthly_limit {
            return CostMonthlyLimit;
        }
        if purpose == MailPurpose::ProjectNews && next >= l.news_pause {
            return CostNewsPaused;
        }
        Reserved
    }
}

#[cfg(test)]
mod tests;
