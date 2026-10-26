//! Infrastructure-free contracts shared by TOR feature packages (I-02).
//!
//! New JSON APIs encode identifiers as decimal strings, preserving PostgreSQL
//! BIGINT values in JavaScript. The legacy public catalog keeps its numeric IDs.
//!
//! Feature identifiers cannot be substituted for one another:
//! ```compile_fail
//! use tor_contracts::{ChapterId, ChapterVersionId};
//! let chapter = ChapterId::try_from(1_i64).unwrap();
//! let edition: ChapterVersionId = chapter;
//! ```
//! ```compile_fail
//! use tor_contracts::{BookId, UserId};
//! fn account(id: UserId) {}
//! account(BookId::try_from(1_i64).unwrap());
//! ```
#![forbid(unsafe_code)]

mod idempotency;
mod ids;

pub use idempotency::{IdempotencyKey, InvalidIdempotencyKey};
pub use ids::{BookId, ChapterId, ChapterVersionId, InvalidId, PositiveId, PurchaseId, UserId};
