//! FLOW-02: bounded, consent-scoped visit windows and finite HTML surfaces.
//! This module never accepts IPs, URLs, cookies, account IDs or search text.
//! Anonymous HTTP pageviews are aggregates, never individual visitors.
use super::{VerifiedSubjectKey, MAX_SUBJECTS};
use std::collections::{BTreeSet, HashMap};
use tor_contracts::flow::{AcquisitionChannel, RouteSurface, VISIT_IDLE_TIMEOUT_SECONDS};

const VISIT_IDLE_MS: u64 = VISIT_IDLE_TIMEOUT_SECONDS * 1_000;
pub const FLOW_SURFACES: usize = 5;
pub const FLOW_SOURCES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowPageview {
    pub surface: RouteSurface,
    /// A coarse request referrer hint, not a proven marketing attribution.
    pub acquisition_hint: AcquisitionChannel,
}

/// No subject key, date, raw referrer or cookie can leave as an aggregate row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowCountRow {
    pub surface: i16,
    pub acquisition_hint: i16,
    pub html_requests: i64,
    pub opted_in_html_requests: i64,
    pub observed_opted_in_visit_starts: i64,
}

/// Read-only hourly aggregate returned by bounded PostgreSQL queries.
/// The hour is a Unix UTC epoch boundary; no individual identity is exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowHourlyCount {
    pub hour_epoch: i64,
    pub surface: i16,
    pub acquisition_hint: i16,
    pub html_requests: i64,
    pub opted_in_html_requests: i64,
    pub observed_opted_in_visit_starts: i64,
}

/// Only the five known HTML surfaces can become audience measurements.
pub fn surface_index(surface: RouteSurface) -> Option<usize> {
    match surface {
        RouteSurface::Home => Some(0),
        RouteSurface::Catalogue => Some(1),
        RouteSurface::BookDetail => Some(2),
        RouteSurface::ChapterReader => Some(3),
        RouteSurface::AuthenticatedPage => Some(4),
        RouteSurface::StaticAsset
        | RouteSurface::Api
        | RouteSurface::Ops
        | RouteSurface::Unknown => None,
    }
}

pub fn acquisition_index(source: AcquisitionChannel) -> usize {
    match source {
        AcquisitionChannel::ExternalOrganicSearch => 0,
        AcquisitionChannel::ExternalReferral => 1,
        AcquisitionChannel::ExternalPaid => 2,
        AcquisitionChannel::DirectOrUnknown => 3,
    }
}

#[derive(Clone, Copy)]
struct LastSeen {
    at_millis: u64,
}

/// The only identified state is bounded in-memory HMAC pseudonyms from a
/// trusted opt-in. No session token or account ID is stored in PostgreSQL.
pub struct FlowVisitWindows {
    max_subjects: usize,
    clock: u64,
    // The first 30 minutes after a restart cannot distinguish a continuing
    // session from a new one. An eviction or opt-out renews that uncertainty.
    // This is a process-wide conservative lower bound, never a unique user ID.
    uncertain_until_millis: u64,
    active: HashMap<VerifiedSubjectKey, LastSeen>,
    recency: BTreeSet<(u64, VerifiedSubjectKey)>,
    evicted: u64,
}

impl FlowVisitWindows {
    pub fn new(max_subjects: usize) -> Self {
        Self {
            max_subjects: max_subjects.clamp(1, MAX_SUBJECTS),
            clock: 0,
            uncertain_until_millis: VISIT_IDLE_MS,
            active: HashMap::new(),
            recency: BTreeSet::new(),
            evicted: 0,
        }
    }

    /// True means a conservatively confirmed start of an opted-in visit,
    /// NOT a distinct person or a site-wide visit. After restart, eviction or
    /// opt-out, unknown starts are suppressed for one idle window (30 minutes).
    /// A deployment with multiple processes still cannot deduplicate visits.
    /// Time is process uptime, not wall clock or a client-supplied timestamp.
    pub fn observe(&mut self, subject: VerifiedSubjectKey, at_millis: u64) -> bool {
        let now = self.clock.max(at_millis);
        self.clock = now;
        self.expire(now);

        let is_new = if let Some(previous) = self.active.remove(&subject) {
            // An active subject cannot have crossed the idle boundary:
            // expire() removes idle entries before this lookup.
            self.recency.remove(&(previous.at_millis, subject));
            false
        } else {
            if self.active.len() >= self.max_subjects {
                if let Some((_, oldest)) = self.recency.pop_first() {
                    self.active.remove(&oldest);
                    self.evicted = self.evicted.saturating_add(1);
                    // The next arrival of the evicted subject might belong
                    // to the same 30-minute visit. No identity is persisted.
                    self.uncertain_until_millis = self
                        .uncertain_until_millis
                        .max(now.saturating_add(VISIT_IDLE_MS));
                }
            }
            now >= self.uncertain_until_millis
        };
        self.active.insert(subject, LastSeen { at_millis: now });
        self.recency.insert((now, subject));
        is_new
    }

    /// Existing anonymous aggregates cannot be reversed, but identified
    /// in-memory state is invalidated synchronously with RIL's opt-out barrier.
    /// The following opt-in epoch cannot be linked back to the old subject;
    /// its first request is therefore uncertain for a complete idle window.
    pub fn forget(&mut self, subject: VerifiedSubjectKey) {
        if let Some(previous) = self.active.remove(&subject) {
            self.recency.remove(&(previous.at_millis, subject));
            self.uncertain_until_millis = self
                .uncertain_until_millis
                .max(self.clock.saturating_add(VISIT_IDLE_MS));
        }
    }

    pub fn active_subjects(&self) -> usize {
        self.active.len()
    }

    pub fn evicted_subjects(&self) -> u64 {
        self.evicted
    }

    fn expire(&mut self, now: u64) {
        while let Some((last, subject)) = self.recency.first().copied() {
            if now.saturating_sub(last) < VISIT_IDLE_MS {
                break;
            }
            self.recency.pop_first();
            self.active.remove(&subject);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(id: u8) -> VerifiedSubjectKey {
        VerifiedSubjectKey::from_hmac_digest([id; 16])
    }

    #[test]
    fn flow_02_one_reader_many_pages_one_visit_then_idle_new_visit() {
        let mut windows = FlowVisitWindows::new(8);
        let id = subject(4);
        assert!(!windows.observe(id, 100));
        assert!(!windows.observe(id, 200));
        assert!(!windows.observe(id, VISIT_IDLE_MS + 199));
        assert!(windows.observe(id, 2 * VISIT_IDLE_MS + 199));
        assert_eq!(windows.active_subjects(), 1);
        // An old completion cannot move the monotonic clock backwards.
        assert!(!windows.observe(id, 1));
    }

    #[test]
    fn flow_02_consent_revocation_forgets_state_without_leaking_key() {
        let mut windows = FlowVisitWindows::new(2);
        let id = subject(9);
        assert!(windows.observe(id, VISIT_IDLE_MS + 50));
        windows.forget(id);
        assert_eq!(windows.active_subjects(), 0);
        // A changed consent epoch is an unknown continuation, not a new visit.
        assert!(!windows.observe(subject(10), VISIT_IDLE_MS + 60));
        assert!(windows.observe(subject(11), 2 * VISIT_IDLE_MS + 61));
        assert_eq!(windows.evicted_subjects(), 0);
        assert!(!format!("{:?}", windows.active_subjects()).contains("subject"));
    }

    #[test]
    fn flow_02_capacity_bounded_and_reappearance_counted_as_uncertain() {
        let mut windows = FlowVisitWindows::new(1);
        assert!(windows.observe(subject(1), VISIT_IDLE_MS + 10));
        // Both first-seen subjects after eviction might be continuing visits.
        assert!(!windows.observe(subject(2), VISIT_IDLE_MS + 11));
        assert_eq!(windows.active_subjects(), 1);
        assert_eq!(windows.evicted_subjects(), 1);
        assert!(!windows.observe(subject(1), VISIT_IDLE_MS + 12));
        assert_eq!(windows.evicted_subjects(), 2);
        // No activity during a full 30-minute grace period makes the next
        // request a new observed session within this single process.
        assert!(windows.observe(subject(3), 2 * VISIT_IDLE_MS + 13));
    }

    #[test]
    fn flow_02_restart_never_recounts_a_continuing_visit() {
        let id = subject(7);
        let mut before_restart = FlowVisitWindows::new(2);
        assert!(before_restart.observe(id, VISIT_IDLE_MS + 100));
        let mut after_restart = FlowVisitWindows::new(2);
        assert!(!after_restart.observe(id, 100));
        assert!(!after_restart.observe(id, VISIT_IDLE_MS - 1));
        assert!(after_restart.observe(id, 2 * VISIT_IDLE_MS + 1));
        assert_eq!(after_restart.active_subjects(), 1);
    }

    #[test]
    fn flow_02_only_bounded_html_categories_are_accepted() {
        for surface in [
            RouteSurface::Home,
            RouteSurface::Catalogue,
            RouteSurface::BookDetail,
            RouteSurface::ChapterReader,
            RouteSurface::AuthenticatedPage,
        ] {
            assert!(surface_index(surface).is_some());
        }
        for surface in [
            RouteSurface::StaticAsset,
            RouteSurface::Api,
            RouteSurface::Ops,
            RouteSurface::Unknown,
        ] {
            assert_eq!(surface_index(surface), None);
        }
        assert_eq!(
            acquisition_index(AcquisitionChannel::DirectOrUnknown),
            FLOW_SOURCES - 1
        );
    }
}
