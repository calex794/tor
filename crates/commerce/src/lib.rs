//! TOR commerce: atomic wallet purchases and evidence-bound external credits.
//! No provider polling or public money-in endpoint is enabled by this package.
#![forbid(unsafe_code)]
pub mod domain;
pub mod repositories;
pub mod services;
