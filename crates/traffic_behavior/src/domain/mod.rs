//! Finite route categories and bounded, non-identifying behavior summaries.
//! Subject keys must be pseudonyms from a trusted, keyed HMAC computed elsewhere.
//! This crate never takes an IP, URI, cookie, account ID, or free-form route label.

mod rollup;
pub use rollup::{RollupDelta, RouteCount, Transition, TransitionCount};

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

pub const MAX_SUBJECTS: usize = 10_000;
/// Temporary in-memory pseudonymous state expires after 30 idle minutes.
pub const SUBJECT_TTL_MS: u64 = 30 * 60 * 1_000;
const ROUTE_COUNT: usize = 15;
const GAP_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RouteCode {
    Home,
    Catalog,
    Books,
    Reader,
    Assets,
    Health,
    Ready,
    Login,
    Register,
    ApiBooks,
    Me,
    Library,
    Read,
    Logout,
    Unknown,
}

impl RouteCode {
    pub fn from_family(family: &str) -> Self {
        match family {
            "/" => Self::Home,
            "/catalog/" => Self::Catalog,
            "/books/" => Self::Books,
            "/reader/" => Self::Reader,
            "/assets/" => Self::Assets,
            "/healthz/" => Self::Health,
            "/readyz/" => Self::Ready,
            "/login/" => Self::Login,
            "/register/" => Self::Register,
            "/api/v1/books/" => Self::ApiBooks,
            "/me/" => Self::Me,
            "/me/library/" => Self::Library,
            "/read/" => Self::Read,
            "/logout/" => Self::Logout,
            _ => Self::Unknown,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StatusGroup {
    Information,
    Success,
    Redirect,
    ClientError,
    ServerError,
    Other,
}

impl StatusGroup {
    pub fn from_status(status: u16) -> Self {
        match status / 100 {
            1 => Self::Information,
            2 => Self::Success,
            3 => Self::Redirect,
            4 => Self::ClientError,
            5 => Self::ServerError,
            _ => Self::Other,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LatencyGroup {
    Under10Ms,
    Under100Ms,
    Under1Sec,
    Slow,
}

impl LatencyGroup {
    pub fn from_elapsed(duration: Duration) -> Self {
        if duration.as_millis() < 10 {
            Self::Under10Ms
        } else if duration.as_millis() < 100 {
            Self::Under100Ms
        } else if duration.as_millis() < 1000 {
            Self::Under1Sec
        } else {
            Self::Slow
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GapBucket {
    Immediate,
    Fast,
    Normal,
    Long,
}

impl GapBucket {
    pub fn from_millis(ms: u64) -> Self {
        match ms {
            0..=199 => Self::Immediate,
            200..=4_999 => Self::Fast,
            5_000..=1_799_999 => Self::Normal,
            _ => Self::Long,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// A digest supplied only by a trusted, authenticated component; never a raw ID.
/// Debug is deliberately not implemented to prevent accidental identifier logging.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VerifiedSubjectKey([u8; 16]);

impl VerifiedSubjectKey {
    pub fn from_hmac_digest(digest: [u8; 16]) -> Self {
        Self(digest)
    }
}

#[derive(Clone, Copy)]
pub struct BehaviorEvent {
    pub route: RouteCode,
    pub status: StatusGroup,
    pub latency: LatencyGroup,
    /// None means anonymous aggregate only: unrelated visitors must not be linked.
    pub subject: Option<VerifiedSubjectKey>,
    /// Monotonic relative uptime, never wall-clock or a session timestamp.
    pub at_millis: u64,
}

#[derive(Clone, Copy)]
struct SubjectState {
    route: RouteCode,
    at_millis: u64,
    touched: u64,
}

/// A snapshot holds no subject keys, raw routes, cookies or user identifiers.
#[derive(Clone)]
pub struct SketchSnapshot {
    pub total: u64,
    pub active_subjects: usize,
    pub evicted_subjects: u64,
    pub dropped_events: u64,
    routes: [u64; ROUTE_COUNT],
    status: [u64; 6],
    latency: [u64; 4],
    transitions: [[[u64; GAP_COUNT]; ROUTE_COUNT]; ROUTE_COUNT],
}

impl SketchSnapshot {
    pub fn count_route(&self, route: RouteCode) -> u64 {
        self.routes[route.index()]
    }

    pub fn count_status(&self, status: StatusGroup) -> u64 {
        self.status[status.index()]
    }

    pub fn count_latency(&self, latency: LatencyGroup) -> u64 {
        self.latency[latency.index()]
    }

    pub fn count_transition(&self, from: RouteCode, to: RouteCode, gap: GapBucket) -> u64 {
        self.transitions[from.index()][to.index()][gap.index()]
    }

    pub fn with_dropped(mut self, dropped: u64) -> Self {
        self.dropped_events = dropped;
        self
    }
}

pub struct BehaviorSketch {
    max_subjects: usize,
    counter: u64,
    last_clock: u64,
    current: HashMap<VerifiedSubjectKey, SubjectState>,
    /// Exactly one entry per subject; unlike append-only LRU it cannot grow forever.
    recency: BTreeSet<(u64, VerifiedSubjectKey)>,
    snapshot: SketchSnapshot,
}

impl BehaviorSketch {
    pub fn new(max_subjects: usize) -> Self {
        Self {
            max_subjects: max_subjects.clamp(1, MAX_SUBJECTS),
            counter: 0,
            last_clock: 0,
            current: HashMap::new(),
            recency: BTreeSet::new(),
            snapshot: SketchSnapshot {
                total: 0,
                active_subjects: 0,
                evicted_subjects: 0,
                dropped_events: 0,
                routes: [0; ROUTE_COUNT],
                status: [0; 6],
                latency: [0; 4],
                transitions: [[[0; GAP_COUNT]; ROUTE_COUNT]; ROUTE_COUNT],
            },
        }
    }

    pub fn record(&mut self, event: BehaviorEvent) -> Option<Transition> {
        let at_millis = event.at_millis.max(self.last_clock);
        self.last_clock = at_millis;
        self.expire_idle(at_millis);
        self.snapshot.total = self.snapshot.total.saturating_add(1);
        self.snapshot.routes[event.route.index()] =
            self.snapshot.routes[event.route.index()].saturating_add(1);
        self.snapshot.status[event.status.index()] =
            self.snapshot.status[event.status.index()].saturating_add(1);
        self.snapshot.latency[event.latency.index()] =
            self.snapshot.latency[event.latency.index()].saturating_add(1);

        // Anonymous traffic increments bounded counters only. No cross-visitor path inference.
        let subject = event.subject?;

        let mut transition = None;

        if let Some(previous) = self.current.get(&subject).copied() {
            self.recency.remove(&(previous.touched, subject));
            let gap = GapBucket::from_millis(at_millis.saturating_sub(previous.at_millis));
            transition = Some(Transition {
                from: previous.route,
                to: event.route,
                gap,
            });
            let slot = &mut self.snapshot.transitions[previous.route.index()][event.route.index()]
                [gap.index()];
            *slot = slot.saturating_add(1);
        } else if self.current.len() >= self.max_subjects {
            if let Some((_, oldest)) = self.recency.pop_first() {
                self.current.remove(&oldest);
                self.snapshot.evicted_subjects = self.snapshot.evicted_subjects.saturating_add(1);
            }
        }

        self.counter = self.counter.saturating_add(1);
        let touched = self.counter;
        self.current.insert(
            subject,
            SubjectState {
                route: event.route,
                at_millis,
                touched,
            },
        );
        self.recency.insert((touched, subject));
        self.snapshot.active_subjects = self.current.len();
        transition
    }

    /// An authenticated opt-out or account deletion can invalidate its pseudonym.
    pub fn forget_subject(&mut self, subject: VerifiedSubjectKey) -> bool {
        let Some(previous) = self.current.remove(&subject) else {
            return false;
        };
        self.recency.remove(&(previous.touched, subject));
        self.snapshot.active_subjects = self.current.len();
        true
    }

    fn expire_idle(&mut self, now: u64) {
        // The recency index is ordered by last touch, so each entry is examined
        // at most once until removal. No O(subjects) scan per HTTP event.
        while let Some((_, key)) = self.recency.first().copied() {
            let Some(state) = self.current.get(&key) else {
                break;
            };
            if now.saturating_sub(state.at_millis) <= SUBJECT_TTL_MS {
                break;
            }
            self.recency.pop_first();
            self.current.remove(&key);
            self.snapshot.evicted_subjects = self.snapshot.evicted_subjects.saturating_add(1);
        }
        self.snapshot.active_subjects = self.current.len();
    }

    pub fn snapshot(&self) -> SketchSnapshot {
        self.snapshot.clone()
    }
}

#[cfg(test)]
mod tests;
