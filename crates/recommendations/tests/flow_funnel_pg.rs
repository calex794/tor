//! F projection over real PostgreSQL canonical C evidence and scope proofs.
//! Actual C command and HTTP capture paths have their own C/web integration suites.
use sqlx::{postgres::PgPoolOptions, PgPool};
use tor_recommendations::{
    domain::feedback::{Feed, FeedItem, OrganicSource, Surface, ALGORITHM},
    repositories::{flow_funnel::PgFlowFunnel, visits::PgFlowVisits, PgRecommendations},
    services::{
        feedback::new_ticket,
        flow_funnel::{FlowFunnelCounts, FlowFunnelError},
        visits::{FlowBookNavigation, FlowScope, FlowVisitError, VisitObservation},
    },
};

#[path = "support/legacy_analytics.rs"]
mod legacy_analytics;

#[path = "flow_funnel_pg/attribution.rs"]
mod attribution;
#[path = "flow_funnel_pg/discovery.rs"]
mod discovery;
#[path = "flow_funnel_pg/fixtures.rs"]
mod fixtures;
#[path = "flow_funnel_pg/lifecycle.rs"]
mod lifecycle;
#[path = "flow_funnel_pg/reconciliation.rs"]
mod reconciliation;
#[path = "flow_funnel_pg/retired.rs"]
mod retired;
use fixtures::*;
