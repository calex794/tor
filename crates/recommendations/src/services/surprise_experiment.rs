//! Account-level ITT, never gift-open attribution. Fixed-horizon Hoeffding
//! intervals cover 18 proportions simultaneously. Safety checks spend alpha
//! over n with 1/(n*(n+1)); repeated looks cannot manufacture efficacy.
pub const MIN_ARM: i64 = 1_000;
pub const PRIVATE_MIN: i64 = 20;
const LOSS: f64 = 0.03;
const DISABLE_INCREASE: f64 = 0.05;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArmCounts {
    pub enrolled: i64,
    pub mature: i64,
    pub meaningful: i64,
    pub baseline_eligible: i64,
    pub current_completed: i64,
    pub returned_d1: i64,
    pub returned_d7: i64,
    pub disabled: i64,
    pub prior_readers: i64,
}

impl ArmCounts {
    fn valid(self) -> bool {
        self.enrolled >= 0
            && (0..=self.enrolled).contains(&self.mature)
            && (0..=self.enrolled).contains(&self.prior_readers)
            && (0..=self.mature).contains(&self.baseline_eligible)
            && (0..=self.baseline_eligible).contains(&self.current_completed)
            && [
                self.meaningful,
                self.returned_d1,
                self.returned_d7,
                self.disabled,
            ]
            .iter()
            .all(|value| (0..=self.mature).contains(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    pub estimate: f64,
    pub lower: f64,
    pub upper: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub code: &'static str,
    pub stop: bool,
    pub new_book_differences: Option<[Difference; 2]>,
}

fn result(code: &'static str, stop: bool) -> Assessment {
    Assessment {
        code,
        stop,
        new_book_differences: None,
    }
}

fn radius(n: i64, sequential: bool) -> f64 {
    if n <= 0 {
        return 1.0;
    }
    let n = n as f64;
    let allocation = if sequential {
        2.0 * 18.0 * n * (n + 1.0) / 0.01
    } else {
        2.0 * 18.0 / 0.05
    };
    (allocation.ln() / (2.0 * n)).sqrt().min(1.0)
}

fn difference(t: i64, tn: i64, c: i64, cn: i64, sequential: bool) -> Difference {
    if tn <= 0 || cn <= 0 {
        return Difference {
            estimate: 0.0,
            lower: -1.0,
            upper: 1.0,
        };
    }
    let estimate = t as f64 / tn as f64 - c as f64 / cn as f64;
    let margin = radius(tn, sequential) + radius(cn, sequential);
    Difference {
        estimate,
        lower: (estimate - margin).max(-1.0),
        upper: (estimate + margin).min(1.0),
    }
}

fn allocation_invalid(arms: &[ArmCounts; 3], total: i64) -> bool {
    if total < MIN_ARM {
        return false;
    }
    let n = total as f64;
    let margin = ((2.0 * 3.0 * n * (n + 1.0) / 0.001).ln() / (2.0 * n)).sqrt();
    arms.iter()
        .zip([0.2, 0.4, 0.4])
        .any(|(arm, expected)| (arm.enrolled as f64 / n - expected).abs() > margin)
}

fn guardrails(control: ArmCounts, treatment: ArmCounts, sequential: bool) -> [Difference; 4] {
    [
        difference(
            treatment.current_completed,
            treatment.baseline_eligible,
            control.current_completed,
            control.baseline_eligible,
            sequential,
        ),
        difference(
            treatment.returned_d1,
            treatment.mature,
            control.returned_d1,
            control.mature,
            sequential,
        ),
        difference(
            treatment.returned_d7,
            treatment.mature,
            control.returned_d7,
            control.mature,
            sequential,
        ),
        difference(
            treatment.disabled,
            treatment.mature,
            control.disabled,
            control.mature,
            sequential,
        ),
    ]
}

/// Order: control, gentle, standard. Non-openers and disablers stay in ITT.
pub fn assess(arms: &[ArmCounts; 3], finished: bool, healthy: bool) -> Assessment {
    if !healthy {
        return result("capture_failure", true);
    }
    let Some(total) = arms.iter().try_fold(0_i64, |n, arm| {
        arm.valid().then(|| n.checked_add(arm.enrolled)).flatten()
    }) else {
        return result("invalid_counts", true);
    };
    if allocation_invalid(arms, total) {
        return result("assignment_mismatch", true);
    }
    if finished && arms.iter().any(|arm| arm.mature != arm.enrolled) {
        return result("incomplete_observation", true);
    }
    if arms.iter().any(|arm| arm.mature < MIN_ARM) {
        return result(
            if finished {
                "insufficient_sample"
            } else {
                "collecting"
            },
            finished,
        );
    }
    let control = arms[0];
    for treatment in &arms[1..] {
        let guards = guardrails(control, *treatment, !finished);
        if guards[..3].iter().any(|delta| delta.upper < -LOSS)
            || guards[3].lower > DISABLE_INCREASE
        {
            return result("guardrail_harm", true);
        }
    }
    if !finished {
        return result("observing", false);
    }
    for treatment in &arms[1..] {
        let baseline = difference(
            treatment.prior_readers,
            treatment.enrolled,
            control.prior_readers,
            control.enrolled,
            false,
        );
        if baseline.lower > 0.0 || baseline.upper < 0.0 {
            return result("baseline_imbalance", true);
        }
    }
    let deltas = [arms[1], arms[2]].map(|treatment| {
        difference(
            treatment.meaningful,
            treatment.mature,
            control.meaningful,
            control.mature,
            false,
        )
    });
    let safe = [arms[1], arms[2]].map(|treatment| {
        let guards = guardrails(control, treatment, false);
        control.baseline_eligible >= PRIVATE_MIN
            && treatment.baseline_eligible >= PRIVATE_MIN
            && guards[..3].iter().all(|delta| delta.lower >= -LOSS)
            && guards[3].upper <= DISABLE_INCREASE
    });
    let supported = [
        deltas[0].lower > 0.0 && safe[0],
        deltas[1].lower > 0.0 && safe[1],
    ];
    let code = match supported {
        [true, true] => "both_supported",
        [true, false] => "gentle_supported",
        [false, true] => "standard_supported",
        [false, false] if deltas.iter().any(|delta| delta.lower > 0.0) => "guardrails_inconclusive",
        _ => "no_demonstrated_gain",
    };
    Assessment {
        code,
        stop: true,
        new_book_differences: Some(deltas),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(n: i64, starts: i64) -> ArmCounts {
        ArmCounts {
            enrolled: n,
            mature: n,
            meaningful: starts,
            baseline_eligible: n / 2,
            current_completed: n * 35 / 100,
            returned_d1: n / 2,
            returned_d7: n / 4,
            disabled: n / 100,
            prior_readers: n / 2,
        }
    }

    fn balanced() -> [ArmCounts; 3] {
        [
            arm(100_000, 10_000),
            arm(200_000, 40_000),
            arm(200_000, 40_000),
        ]
    }

    #[test]
    fn positive_result_requires_horizon_and_all_guardrails() {
        let arms = balanced();
        assert_eq!(assess(&arms, false, true).code, "observing");
        let accepted = assess(&arms, true, true);
        assert_eq!(accepted.code, "both_supported");
        assert!(accepted.new_book_differences.unwrap()[0].lower > 0.0);
    }

    #[test]
    fn failures_never_look_like_zero_success() {
        assert_eq!(assess(&balanced(), true, false).code, "capture_failure");
        let mut arms = balanced();
        arms[1].meaningful = arms[1].mature + 1;
        assert_eq!(assess(&arms, true, true).code, "invalid_counts");
    }

    #[test]
    fn mature_window_and_minimum_sample_are_required() {
        let mut arms = balanced();
        arms[1].mature -= 1;
        assert_eq!(assess(&arms, true, true).code, "incomplete_observation");
        let small = [arm(100, 10), arm(200, 40), arm(200, 40)];
        assert_eq!(assess(&small, true, true).code, "insufficient_sample");
        assert!(assess(&small, true, true).new_book_differences.is_none());
    }

    #[test]
    fn imbalance_and_obvious_harm_stop_the_trial() {
        let unbalanced = [arm(100_000, 10_000); 3];
        assert_eq!(assess(&unbalanced, false, true).code, "assignment_mismatch");
        let mut arms = balanced();
        arms[1].returned_d7 = 0;
        assert_eq!(assess(&arms, false, true).code, "guardrail_harm");
        arms = balanced();
        arms[2].disabled = 100_000;
        assert_eq!(assess(&arms, false, true).code, "guardrail_harm");
    }

    #[test]
    fn primary_gain_does_not_replace_completion_guardrail() {
        let mut arms = balanced();
        for arm in &mut arms {
            arm.baseline_eligible = 0;
            arm.current_completed = 0;
        }
        assert_eq!(assess(&arms, true, true).code, "guardrails_inconclusive");
    }

    #[test]
    fn zero_events_and_baseline_imbalance_do_not_prove_uplift() {
        let mut arms = balanced();
        for arm in &mut arms {
            arm.meaningful = 0;
        }
        assert_eq!(assess(&arms, true, true).code, "no_demonstrated_gain");
        arms = balanced();
        arms[1].prior_readers = 0;
        assert_eq!(assess(&arms, true, true).code, "baseline_imbalance");
    }

    #[test]
    fn intervals_are_bounded_and_sequential_is_more_conservative() {
        let fixed = difference(0, 1_000, 1_000, 1_000, false);
        let sequential = difference(0, 1_000, 1_000, 1_000, true);
        assert_eq!(fixed.lower, -1.0);
        assert!(sequential.upper > fixed.upper);
        assert!(radius(0, false).is_finite());
    }
}
