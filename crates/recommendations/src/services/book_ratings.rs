//! A-04/F-05: session-scoped reading and saving of two independent ratings.
#![forbid(unsafe_code)]
use crate::repositories::{PgBookStars, StarKind};
pub use crate::repositories::StarRatingError;

/// HTTP view data does not expose PostgreSQL repository records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RatingPage {
    pub title: String,
    pub work: Option<i16>,
    pub translation: Option<i16>,
    pub eligible: bool,
}

/// A validated value, constructed before HTTP rate-limit consumption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedRating {
    kind: StarKind,
    score: i16,
}

#[derive(Clone)]
pub struct BookRatingService {
    repository: PgBookStars,
}

impl BookRatingService {
    pub fn new(repository: PgBookStars) -> Self {
        Self { repository }
    }

    pub fn for_actor(&self, actor: i64) -> Result<ActorRatings<'_>, StarRatingError> {
        if actor <= 0 {
            return Err(StarRatingError::Invalid);
        }
        Ok(ActorRatings {
            repository: &self.repository,
            actor,
        })
    }

    pub fn validate_submission(
        rating_kind: &str,
        score: i16,
    ) -> Result<ValidatedRating, StarRatingError> {
        let kind = StarKind::parse(rating_kind).ok_or(StarRatingError::Invalid)?;
        if !(1..=5).contains(&score) {
            return Err(StarRatingError::Invalid);
        }
        Ok(ValidatedRating { kind, score })
    }
}

pub struct ActorRatings<'a> {
    repository: &'a PgBookStars,
    actor: i64,
}

impl ActorRatings<'_> {
    /// A visible Book is required before loading personal choices or eligibility.
    /// The repository independently rechecks current visibility and age access.
    pub async fn page(&self, book_id: i64) -> Result<Option<RatingPage>, StarRatingError> {
        let Some(summary) = self
            .repository
            .visible_summary_for_actor(book_id, Some(self.actor))
            .await?
        else {
            return Ok(None);
        };
        let own = self.repository.own_ratings(self.actor, book_id).await?;
        let eligible = self.repository.eligible(self.actor, book_id).await?;
        Ok(Some(RatingPage {
            title: summary.title,
            work: own.work,
            translation: own.translation,
            eligible,
        }))
    }

    /// No read-side eligibility flag is trusted: the PG UPSERT checks active
    /// account, Book visibility, age rights and completed chapter again.
    pub async fn save(
        &self,
        book_id: i64,
        rating: ValidatedRating,
    ) -> Result<(), StarRatingError> {
        self.repository
            .set_rating(self.actor, book_id, rating.kind, rating.score)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a04_accepts_only_two_rating_kinds_and_1_to_5() {
        assert_eq!(
            BookRatingService::validate_submission("work", 1),
            Ok(ValidatedRating {
                kind: StarKind::Work,
                score: 1
            })
        );
        assert_eq!(
            BookRatingService::validate_submission("translation", 5),
            Ok(ValidatedRating {
                kind: StarKind::Translation,
                score: 5
            })
        );
        for kind in ["", "WORK", "rank"] {
            assert_eq!(
                BookRatingService::validate_submission(kind, 3),
                Err(StarRatingError::Invalid)
            );
        }
        for score in [-1, 0, 6] {
            assert_eq!(
                BookRatingService::validate_submission("work", score),
                Err(StarRatingError::Invalid)
            );
        }
    }
}
