//! Reader-visible state of a request for more chapters.
//! This state does not grant publication or reading rights.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemandState {
    NotEligible,
    Ready,
    Recorded,
}
