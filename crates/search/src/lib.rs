//! F-03: fail-closed title search of public books only.
pub mod api;
pub mod domain;
pub mod repositories;
pub mod services;

pub use domain::{SearchBook, SearchError, SearchPage};
pub use repositories::PgSearch;
