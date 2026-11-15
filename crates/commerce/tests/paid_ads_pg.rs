//! ADS money acceptance uses only a freshly migrated, disposable PostgreSQL.
#![forbid(unsafe_code)]
mod paid_ads {
    pub mod fixture;
    mod lifecycle;
    mod purchase;
    mod safety;
}
