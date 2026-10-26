//! Bounded technical traffic sketches. No identities or business decisions.
#![forbid(unsafe_code)]

pub mod domain;
pub mod services;

pub use domain::{
    BehaviorEvent, BehaviorSketch, GapBucket, LatencyGroup, RouteCode, SketchSnapshot,
    StatusGroup, VerifiedSubjectKey, MAX_SUBJECTS,
};
pub use services::TrafficRecorder;
