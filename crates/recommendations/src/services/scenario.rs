//! Pinned NEwBOO 17125ef scenario_detection_service.py weights and precedence.
use super::hybrid_scoring::Weights;

#[derive(Default)]
pub struct ScenarioFacts {
    pub recent_books: usize,
    pub recent_signals: usize,
    pub behavioral_candidates: usize,
    pub source_edges: usize,
    pub session_events: usize,
    pub source_age_days: Option<i64>,
}
pub fn weights(f: &ScenarioFacts) -> Weights {
    let (behavioral, content, popularity) = if f.recent_books == 0 && f.recent_signals == 0 {
        (0, 40, 60)
    } else if f.session_events >= 5 {
        (70, 20, 10)
    } else if f.source_age_days.is_some_and(|age| age <= 14) {
        (10, 60, 30)
    } else if f.behavioral_candidates <= 2 && f.source_edges <= 2 {
        (10, 50, 40)
    } else if f.recent_books >= 3 && f.recent_signals >= 3 {
        (60, 30, 10)
    } else {
        (50, 40, 10)
    };
    Weights {
        behavioral,
        content,
        popularity,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_scenario_precedence_and_weight_sum() {
        let mut f = ScenarioFacts::default();
        let w = weights(&f);
        assert_eq!((w.behavioral, w.content, w.popularity), (0, 40, 60));
        f.recent_books = 3;
        f.recent_signals = 3;
        let w = weights(&f);
        assert_eq!((w.behavioral, w.content, w.popularity), (10, 50, 40));
        f.behavioral_candidates = 3;
        f.source_edges = 3;
        let w = weights(&f);
        assert_eq!((w.behavioral, w.content, w.popularity), (60, 30, 10));
        f.source_age_days = Some(14);
        let w = weights(&f);
        assert_eq!((w.behavioral, w.content, w.popularity), (10, 60, 30));
        f.session_events = 5;
        let w = weights(&f);
        assert_eq!((w.behavioral, w.content, w.popularity), (70, 20, 10));
        assert_eq!(w.behavioral + w.content + w.popularity, 100);
    }
}
