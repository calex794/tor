//! F-owned PostgreSQL adapters; reading and financial authority stay in C/D.
pub mod achievements;
pub use achievements::{AchievementStoreError, PgAchievements, ReaderAchievements, ReaderBadge};
mod book_stars;
pub use book_stars::{BookStars, PgBookStars, StarKind, StarRatingError, StarTotal, UserStars};
mod completion;
pub mod postgres;
pub use postgres::{PgRanking, RankingCommand, RankingCommit, RankingStoreError, VisibleRank};
pub mod feedback;
pub use feedback::PgRecommendations;
