//! Full C ranking and bounded independent F/C diagnostics through production HTTP.
#![forbid(unsafe_code)]
#[path = "catalog_interest_http_pg/fixture.rs"]
mod fixture;
#[path = "catalog_interest_http_pg/ranking.rs"]
mod ranking;
#[path = "catalog_interest_http_pg/reading.rs"]
mod reading;
