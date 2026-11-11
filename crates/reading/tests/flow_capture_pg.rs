//! Legacy FLOW-03 capture regressions on the explicit pre-retirement 191 schema.
//! Run serially because the outage cases deliberately lock the capture tables.
use sqlx::{postgres::PgPoolOptions, PgPool};
use tor_reading::{
    repositories::{
        flow_capture::PgFlowCapture, sync::PurchasedFinish, PgBookFollows, PgLibrary, PgReader,
        PgReadingProgress, PgReadingSync,
    },
    services::{progress::ReadingPlace, sync::SyncError, PublicReadingRepository},
};

#[path = "flow_capture_pg/actions.rs"]
mod actions;
#[path = "flow_capture_pg/activity.rs"]
mod activity;
#[path = "flow_capture_pg/failures.rs"]
mod failures;
#[path = "flow_capture_pg/fixtures.rs"]
mod fixtures;
#[path = "flow_capture_pg/read_receipts.rs"]
mod read_receipts;
use fixtures::*;

#[path = "support/legacy_analytics.rs"]
mod legacy;
