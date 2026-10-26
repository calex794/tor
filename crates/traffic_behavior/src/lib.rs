//! Bounded technical traffic sketches. No identities or business decisions.
#![forbid(unsafe_code)]

pub mod domain;
pub mod repositories;
pub mod services;

pub use domain::{
    BehaviorEvent, BehaviorSketch, GapBucket, Hygiene, LatencyGroup, PathAssessment, PathReport,
    RollupDelta, RouteCode, SketchSnapshot, SqlCostCount, SqlCostEvent, StatusGroup, Transition,
    VerifiedSubjectKey,
    MAX_SUBJECTS, PATH_VERSION, PATH_WINDOW,
};
pub use services::TrafficRecorder;
