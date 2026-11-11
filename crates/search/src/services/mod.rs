//! Search semantics are deliberately read-only. Repository owns all SQL.
pub mod seo_analytics;
pub const MAX_QUERY_LENGTH: usize = 80;
pub mod catalog_taxonomy;
pub mod seo;
pub mod showcases;
