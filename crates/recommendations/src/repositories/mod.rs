//! Private F-owned PostgreSQL records; never a public popularity projection.
mod book_stars;
pub use book_stars::{BookStars, PgBookStars, StarKind, StarRatingError, StarTotal, UserStars};
mod completion;
pub mod postgres;
pub use postgres::{PgRanking, RankingCommand, RankingCommit, RankingStoreError, VisibleRank};
