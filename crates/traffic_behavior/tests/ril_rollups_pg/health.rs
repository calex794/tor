//! Real RIL materialization is the acknowledgement; a heartbeat alone is not.
use super::observed::Fixture;
use sqlx::Row;
use std::time::{Duration, Instant};
use tor_contracts::flow::{AcquisitionChannel, RouteSurface};
use tor_traffic_behavior::{
    domain::{
        html_health::{HtmlHealthError, HTML_CLOCK_MARGIN_MS},
        RollupDelta,
    },
    repositories::{PgFlowAudience, PgHtmlHealth, PgRilRollups},
    services::{HtmlIngressStopped, PgRollupPump},
    BehaviorEvent, FlowPageview, LatencyGroup, RouteCode, StatusGroup, TrafficRecorder,
};
use uuid::Uuid;

fn event() -> BehaviorEvent {
    BehaviorEvent {
        route: RouteCode::Books,
        status: StatusGroup::Success,
        latency: LatencyGroup::Under10Ms,
        subject: None,
        at_millis: 1,
    }
}
fn page() -> FlowPageview {
    FlowPageview {
        surface: RouteSurface::BookDetail,
        acquisition_hint: AcquisitionChannel::DirectOrUnknown,
    }
}

#[path = "health/batches.rs"]
mod batches;
#[path = "health/capability.rs"]
mod capability;
#[path = "health/epochs.rs"]
mod epochs;
#[path = "health/lifecycle.rs"]
mod lifecycle;
#[path = "health/retention.rs"]
mod retention;

#[path = "health/cancellation.rs"]
mod cancellation;
