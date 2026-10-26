//! RIL-07: bounded, explainable path analysis off the HTTP request path.
//! Only trusted HMAC pseudonyms and finite route/status codes enter this model.
//! No IP, raw URL, user ID, book ID, cookie or precise wall-clock timestamp.
use super::{
    BehaviorEvent, GapBucket, RouteCode, StatusGroup, VerifiedSubjectKey,
    MAX_SUBJECTS, SUBJECT_TTL_MS,
};
use std::collections::{BTreeSet, HashMap, VecDeque};

pub const PATH_VERSION: &str = "ril_paths_v1";
pub const PATH_WINDOW: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hygiene {
    Normal,
    Watch,
    Degraded,
    Reject,
}

/// 'Reject' is a diagnostic quality bucket. It never authorizes blocking.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathReasons {
    pub repeated_loop: bool,
    pub backtracking: bool,
    pub unknown_probes: bool,
    pub error_burst: bool,
    pub rapid_unknowns: bool,
}

/// No identifier is returned with the diagnostic; callers must not turn
/// a heuristic into an ACL, fraud, billing or recommendation decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathAssessment {
    pub version: &'static str,
    pub hygiene: Hygiene,
    pub events: u8,
    pub loops: u8,
    pub backtracks: u8,
    pub unknown: u8,
    pub errors: u8,
    pub rapid_unknowns: u8,
    pub reasons: PathReasons,
}

#[derive(Clone, Copy)]
struct Point {
    route: RouteCode,
    status: StatusGroup,
    gap: GapBucket,
}

struct SubjectPath {
    points: VecDeque<Point>,
    last_millis: u64,
    touched: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathReport {
    /// Number of assessments, NOT number of distinct people.
    pub assessed_events: u64,
    pub active_subjects: usize,
    pub evicted_subjects: u64,
    pub dropped_events: u64,
    pub normal: u64,
    pub watch: u64,
    pub degraded: u64,
    pub reject: u64,
    pub unknown_probes: u64,
    pub backtracks: u64,
}

impl PathReport {
    pub fn with_dropped(mut self, count: u64) -> Self {
        self.dropped_events = count;
        self
    }
}

pub struct PathAnalyzer {
    capacity: usize,
    clock: u64,
    touches: u64,
    subjects: HashMap<VerifiedSubjectKey, SubjectPath>,
    recency: BTreeSet<(u64, VerifiedSubjectKey)>,
    report: PathReport,
}

impl PathAnalyzer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, MAX_SUBJECTS),
            clock: 0,
            touches: 0,
            subjects: HashMap::new(),
            recency: BTreeSet::new(),
            report: PathReport::default(),
        }
    }

    pub fn record(&mut self, event: BehaviorEvent) -> Option<PathAssessment> {
        // Anonymous visitors do not form artificial shared "user paths".
        let subject = event.subject?;
        let now = event.at_millis.max(self.clock);
        self.clock = now;
        self.expire_idle(now);
        let old = self.subjects.get(&subject);
        let gap = old.map(|state| GapBucket::from_millis(
            now.saturating_sub(state.last_millis),
        )).unwrap_or(GapBucket::Long);
        if let Some(old) = old {
            self.recency.remove(&(old.touched, subject));
        } else if self.subjects.len() == self.capacity {
            self.evict_oldest();
        }

        self.touches = self.touches.saturating_add(1);
        let touched = self.touches;
        let state = self.subjects.entry(subject).or_insert_with(|| SubjectPath {
            points: VecDeque::with_capacity(PATH_WINDOW),
            last_millis: now,
            touched,
        });
        state.points.push_back(Point {
            route: event.route,
            status: event.status,
            gap,
        });
        if state.points.len() > PATH_WINDOW {
            state.points.pop_front();
        }
        state.last_millis = now;
        state.touched = touched;
        self.recency.insert((touched, subject));
        let result = Self::assess(&state.points);

        self.report.assessed_events = self.report.assessed_events.saturating_add(1);
        self.report.active_subjects = self.subjects.len();
        match result.hygiene {
            Hygiene::Normal => self.report.normal = self.report.normal.saturating_add(1),
            Hygiene::Watch => self.report.watch = self.report.watch.saturating_add(1),
            Hygiene::Degraded => self.report.degraded = self.report.degraded.saturating_add(1),
            Hygiene::Reject => self.report.reject = self.report.reject.saturating_add(1),
        };
        if result.reasons.unknown_probes {
            self.report.unknown_probes = self.report.unknown_probes.saturating_add(1);
        }
        if result.reasons.backtracking {
            self.report.backtracks = self.report.backtracks.saturating_add(1);
        }
        Some(result)
    }

    fn assess(points: &VecDeque<Point>) -> PathAssessment {
        let mut loops: u8 = 0;
        let mut backtracks: u8 = 0;
        let mut unknown: u8 = 0;
        let mut errors: u8 = 0;
        let mut rapid_unknowns: u8 = 0;
        let mut run: u8 = 0;
        let mut last: Option<RouteCode> = None;
        let mut two_back: Option<RouteCode> = None;
        for point in points {
            let reader_route = matches!(point.route, RouteCode::Reader | RouteCode::Read);
            if point.route == RouteCode::Unknown {
                unknown = unknown.saturating_add(1);
                if matches!(point.gap, GapBucket::Immediate | GapBucket::Fast) {
                    rapid_unknowns = rapid_unknowns.saturating_add(1);
                }
            }
            if matches!(point.status, StatusGroup::ClientError | StatusGroup::ServerError) {
                errors = errors.saturating_add(1);
            }
            if last == Some(point.route) {
                run = run.saturating_add(1);
                if run >= 3 && !reader_route {
                    loops = loops.saturating_add(1);
                }
            } else {
                run = 1;
            }
            if two_back == Some(point.route) && last != Some(point.route) && !reader_route {
                backtracks = backtracks.saturating_add(1);
            }
            two_back = last;
            last = Some(point.route);
        }
        // A long reader pause or repeated chapter navigation alone is normal.
        // Every elevated rating requires several independent weak signals.
        let hygiene = if points.len() >= 16 && unknown >= 12
            && rapid_unknowns >= 9 && errors >= 9
        {
            Hygiene::Reject
        } else if (unknown >= 6 && rapid_unknowns >= 4 && errors >= 4)
            || (backtracks >= 8 && errors >= 5)
        {
            Hygiene::Degraded
        } else if (unknown >= 3 && errors >= 2)
            || (backtracks >= 5 && errors >= 2)
            || (loops >= 8 && errors >= 4)
        {
            Hygiene::Watch
        } else {
            Hygiene::Normal
        };
        PathAssessment {
            version: PATH_VERSION,
            hygiene,
            events: points.len() as u8,
            loops,
            backtracks,
            unknown,
            errors,
            rapid_unknowns,
            reasons: PathReasons {
                repeated_loop: loops >= 3,
                backtracking: backtracks >= 2,
                unknown_probes: unknown >= 3,
                error_burst: errors >= 3,
                rapid_unknowns: rapid_unknowns >= 3,
            },
        }
    }

    fn evict_oldest(&mut self) {
        if let Some((_, key)) = self.recency.pop_first() {
            self.subjects.remove(&key);
            self.report.evicted_subjects = self.report.evicted_subjects.saturating_add(1);
        }
    }

    fn expire_idle(&mut self, now: u64) {
        while let Some((_, key)) = self.recency.first().copied() {
            let Some(subject) = self.subjects.get(&key) else {
                break;
            };
            if now.saturating_sub(subject.last_millis) <= SUBJECT_TTL_MS {
                break;
            }
            self.evict_oldest();
        }
        self.report.active_subjects = self.subjects.len();
    }

    /// Trusted opt-out/reset: remove the pseudonymous sequence, not the
    /// already aggregated, non-identifying global diagnostics.
    pub fn forget_subject(&mut self, key: VerifiedSubjectKey) -> bool {
        let Some(state) = self.subjects.remove(&key) else {
            return false;
        };
        self.recency.remove(&(state.touched, key));
        self.report.active_subjects = self.subjects.len();
        true
    }

    pub fn report(&self) -> PathReport {
        self.report
    }
}

#[cfg(test)]
mod tests;
