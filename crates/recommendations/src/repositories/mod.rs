//! Private F-owned PostgreSQL records; never a public popularity projection.
pub mod postgres;
mod completion;
pub use postgres::{PgRanking, RankingCommand, RankingCommit, RankingStoreError, VisibleRank};
