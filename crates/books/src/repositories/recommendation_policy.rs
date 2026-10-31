//! B-owned recommendation audience and manual relations. Unknown/adult stays
//! ineligible until a separately verified adult-access policy is available.
use sqlx::{PgPool, Row};
use tor_contracts::auth::SessionActor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecommendationPolicyError {
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
}
impl From<sqlx::Error> for RecommendationPolicyError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
#[derive(Debug)]
pub struct BookRecommendationSettings {
    pub book_id: i64,
    pub title: String,
    pub audience: String,
    pub revision: i64,
    pub reviewed: bool,
    pub related: Vec<i64>,
}
#[derive(Clone)]
pub struct PgRecommendationPolicy {
    pool: PgPool,
}
impl PgRecommendationPolicy {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// SessionActor is supplied by A, never deserialized from a browser body.
    pub async fn reviewable_books(
        &self,
        actor: SessionActor,
        after: i64,
    ) -> Result<Vec<(i64, String)>, RecommendationPolicyError> {
        if after < 0 {
            return Err(RecommendationPolicyError::Invalid);
        }
        let rows=sqlx::query("SELECT id,title FROM books WHERE (owner_user_id=$1 OR $2) AND NOT is_deleted AND id>$3 ORDER BY id LIMIT 20")
            .bind(actor.actor.user_id.get()).bind(actor.administrator).bind(after).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| Ok((r.try_get("id")?, r.try_get("title")?)))
            .collect()
    }

    pub async fn recommendation_settings(
        &self,
        actor: SessionActor,
        book: i64,
    ) -> Result<BookRecommendationSettings, RecommendationPolicyError> {
        if book <= 0 {
            return Err(RecommendationPolicyError::Invalid);
        }
        let row=sqlx::query("SELECT b.title,COALESCE(a.audience,'unreviewed') AS audience,COALESCE(a.revision,0) AS revision,(review.reviewer_user_id IS NOT NULL AND review.reviewed_revision=a.revision) AS reviewed FROM books b LEFT JOIN book_recommendation_audiences a ON a.book_id=b.id LEFT JOIN rec_general_audience_approvals review ON review.book_id=b.id WHERE b.id=$1 AND (b.owner_user_id=$2 OR $3) AND NOT b.is_deleted")
            .bind(book).bind(actor.actor.user_id.get()).bind(actor.administrator).fetch_optional(&self.pool).await?.ok_or(RecommendationPolicyError::NotFound)?;
        let related=sqlx::query_scalar("SELECT r.related_book_id FROM book_recommendation_relations r JOIN rec_public_books b ON b.id=r.related_book_id WHERE r.book_id=$1 ORDER BY r.related_book_id LIMIT 10")
            .bind(book).fetch_all(&self.pool).await?;
        Ok(BookRecommendationSettings {
            book_id: book,
            title: row.try_get("title")?,
            audience: row.try_get("audience")?,
            revision: row.try_get("revision")?,
            reviewed: row.try_get("reviewed")?,
            related,
        })
    }

    pub async fn update_recommendation_settings(
        &self,
        actor: SessionActor,
        book: i64,
        expected: i64,
        audience: &str,
        related: &[i64],
    ) -> Result<i64, RecommendationPolicyError> {
        if book <= 0
            || !(0..i64::MAX).contains(&expected)
            || !["general", "adult", "unreviewed"].contains(&audience)
            || related.len() > 10
            || related.iter().any(|id| *id <= 0 || *id == book)
        {
            return Err(RecommendationPolicyError::Invalid);
        }
        let mut unique = related.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != related.len() {
            return Err(RecommendationPolicyError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let owner:Option<i64>=sqlx::query_scalar("SELECT id FROM books WHERE id=$1 AND (owner_user_id=$2 OR $3) AND NOT is_deleted FOR UPDATE")
            .bind(book).bind(actor.actor.user_id.get()).bind(actor.administrator).fetch_optional(&mut *tx).await?;
        if owner.is_none() {
            return Err(RecommendationPolicyError::NotFound);
        }
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM book_recommendation_audiences WHERE book_id=$1",
        )
        .bind(book)
        .fetch_optional(&mut *tx)
        .await?;
        if revision.unwrap_or(0) != expected {
            return Err(RecommendationPolicyError::Conflict);
        }
        let eligible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rec_public_books WHERE id=ANY($1::bigint[])")
                .bind(related)
                .fetch_one(&mut *tx)
                .await?;
        if eligible != related.len() as i64 {
            return Err(RecommendationPolicyError::NotFound);
        }
        sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience,revision) VALUES($1,$2,$3) ON CONFLICT(book_id) DO UPDATE SET audience=EXCLUDED.audience,revision=EXCLUDED.revision,updated_at=now()")
            .bind(book).bind(audience).bind(expected+1).execute(&mut *tx).await?;
        // Every edit invalidates the earlier approval; only an A-verified
        // administrator can confirm general eligibility at the new CAS revision.
        sqlx::query("DELETE FROM rec_general_audience_approvals WHERE book_id=$1")
            .bind(book).execute(&mut *tx).await?;
        if actor.administrator && audience == "general" {
            sqlx::query("INSERT INTO rec_general_audience_approvals(book_id,reviewed_revision,reviewer_user_id) VALUES($1,$2,$3)")
                .bind(book).bind(expected+1).bind(actor.actor.user_id.get())
                .execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM book_recommendation_relations WHERE book_id=$1")
            .bind(book)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO book_recommendation_relations(book_id,related_book_id) SELECT $1,unnest($2::bigint[])")
            .bind(book).bind(related).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(expected + 1)
    }
}
