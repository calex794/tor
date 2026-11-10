//! Retired anonymous evidence must not become a partial numeric total or zero.
//! Historical read snapshots are isolated; the adapter test uses I's real port.
#[path = "support/current_analytics.rs"]
mod current_analytics;
#[path = "analytics_invalidation_pg/books.rs"]
mod books;
#[path = "analytics_invalidation_pg/fixtures.rs"]
mod fixtures;
#[path = "analytics_invalidation_pg/groups.rs"]
mod groups;
#[path = "analytics_invalidation_pg/retirement.rs"]
mod retirement;
#[path = "analytics_invalidation_pg/sources.rs"]
mod sources;
