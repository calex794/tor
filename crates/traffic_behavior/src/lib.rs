//! Bounded technical traffic sketches. No identities or business decisions.
#![forbid(unsafe_code)]

pub mod domain;
pub mod repositories;
pub mod services;

pub use domain::{
    BehaviorEvent, BehaviorSketch, GapBucket, LatencyGroup, RollupDelta, RouteCode, SketchSnapshot,
    StatusGroup, Transition, VerifiedSubjectKey, MAX_SUBJECTS,
};
pub use services::TrafficRecorder;
