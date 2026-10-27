//! Private F-owned PostgreSQL records; never a public popularity projection.
mod completion;
pub mod postgres;
pub use postgres::{PgRanking, RankingCommand, RankingCommit, RankingStoreError, VisibleRank};
