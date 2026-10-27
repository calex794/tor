//! Private F-owned PostgreSQL records; never a public popularity projection.
pub mod postgres;
pub use postgres::{PgRanking, RankingCommand, RankingCommit, RankingStoreError, VisibleRank};
