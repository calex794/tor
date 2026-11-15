//! FIN-A01/A02: cumulative minor-unit accounting, including zero-cost events.
use tor_contracts::{ads::AdError, money::MoneyMinor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdPricing {
    budget: MoneyMinor,
    target: i64,
    publisher_bps: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdAccrual {
    pub gross: MoneyMinor,
    pub publisher: MoneyMinor,
    pub platform: MoneyMinor,
    pub unused: MoneyMinor,
}
impl AdPricing {
    pub fn new(budget: MoneyMinor, target: i64, publisher_bps: i64) -> Result<Self, AdError> {
        if budget.get() <= 0 || target <= 0 || !(0..=10_000).contains(&publisher_bps) {
            return Err(AdError::Invalid);
        }
        Ok(Self {
            budget,
            target,
            publisher_bps,
        })
    }
    pub fn cumulative(self, count: i64) -> Result<AdAccrual, AdError> {
        if count < 0 || count > self.target {
            return Err(AdError::Invalid);
        }
        let gross = i128::from(self.budget.get()) * i128::from(count) / i128::from(self.target);
        let publisher = gross * i128::from(self.publisher_bps) / 10_000;
        let money = |v| {
            i64::try_from(v)
                .ok()
                .and_then(|v| MoneyMinor::new(v).ok())
                .ok_or(AdError::Invalid)
        };
        Ok(AdAccrual {
            gross: money(gross)?,
            publisher: money(publisher)?,
            platform: money(gross - publisher)?,
            unused: money(i128::from(self.budget.get()) - gross)?,
        })
    }
    pub fn next(self, ordinal: i64) -> Result<AdAccrual, AdError> {
        if ordinal <= 0 {
            return Err(AdError::Invalid);
        }
        let after = self.cumulative(ordinal)?;
        let before = self.cumulative(ordinal - 1)?;
        Ok(AdAccrual {
            gross: after
                .gross
                .checked_sub(before.gross)
                .map_err(|_| AdError::Invalid)?,
            publisher: after
                .publisher
                .checked_sub(before.publisher)
                .map_err(|_| AdError::Invalid)?,
            platform: after
                .platform
                .checked_sub(before.platform)
                .map_err(|_| AdError::Invalid)?,
            unused: after.unused,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fin_a01_exact_remainder_and_refund() {
        let p = AdPricing::new(MoneyMinor::new(101).unwrap(), 4, 7000).unwrap();
        let rows: Vec<_> = (1..=4).map(|k| p.next(k).unwrap()).collect();
        assert_eq!(
            rows.iter().map(|v| v.gross.get()).collect::<Vec<_>>(),
            [25, 25, 25, 26]
        );
        assert_eq!(
            rows.iter().map(|v| v.publisher.get()).collect::<Vec<_>>(),
            [17, 18, 17, 18]
        );
        let half = p.cumulative(2).unwrap();
        assert_eq!(
            (half.unused.get(), half.publisher.get(), half.platform.get()),
            (51, 35, 15)
        );
    }
    #[test]
    fn fin_a02_sampled_prefix_monotonicity_and_exact_telescoping() {
        let mut state = 0x48ad_0420_0430_0001_u64;
        for _ in 0..10_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let budget = i64::try_from((state >> 1).max(1)).unwrap();
            let target = i64::try_from(1 + state % 500).unwrap();
            let bps = i64::try_from(state % 10001).unwrap();
            let k = i64::try_from(state % u64::try_from(target).unwrap()).unwrap();
            let p = AdPricing::new(MoneyMinor::new(budget).unwrap(), target, bps).unwrap();
            let before = p.cumulative(k).unwrap();
            let after = p.cumulative(k + 1).unwrap();
            let delta = p.next(k + 1).unwrap();
            assert!(
                after.gross.get() >= before.gross.get()
                    && after.publisher.get() >= before.publisher.get()
                    && after.platform.get() >= before.platform.get()
            );
            assert_eq!(before.gross.get() + delta.gross.get(), after.gross.get());
            assert_eq!(
                delta.publisher.get() + delta.platform.get(),
                delta.gross.get()
            );
            assert_eq!(after.gross.get() + after.unused.get(), budget);
            assert_eq!(p.cumulative(target).unwrap().gross.get(), budget);
        }
    }
    #[test]
    fn fin_a02_cumulative_invariants_and_maximum_integer() {
        for budget in [1, 2, 99, 101, 10_000, i64::MAX] {
            for target in [1, 2, 4, 101, 500, i64::MAX] {
                for bps in [0, 1, 3333, 7000, 9999, 10_000] {
                    let p = AdPricing::new(MoneyMinor::new(budget).unwrap(), target, bps).unwrap();
                    for k in [0, 1, target / 2, target] {
                        let a = p.cumulative(k).unwrap();
                        assert_eq!(a.gross.get() + a.unused.get(), budget);
                        assert_eq!(a.publisher.get() + a.platform.get(), a.gross.get());
                        if k == target {
                            assert_eq!(a.gross.get(), budget);
                        }
                    }
                    if target <= 500 {
                        let sum: i64 = (1..=target).map(|k| p.next(k).unwrap().gross.get()).sum();
                        assert_eq!(sum, budget);
                    }
                }
            }
        }
        assert!(AdPricing::new(MoneyMinor::new(1).unwrap(), 0, 1).is_err());
        assert!(AdPricing::new(MoneyMinor::new(0).unwrap(), 1, 1).is_err());
    }
}
