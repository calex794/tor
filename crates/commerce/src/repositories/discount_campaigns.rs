//! D-07: owner-only campaigns. Money and subscriber eligibility stay in D.
//! Existing E creator-follow periods are read-only here, never modified.
#![forbid(unsafe_code)]

use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgAuthorDiscounts {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DiscountCampaignError {
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActiveAuthorDiscount {
    pub id: i64,
    pub book_id: i64,
    pub discount_percent: i16,
    pub min_follow_days: i16,
    pub ends_on: String,
}

impl PgAuthorDiscounts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn owned_book(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        owner: i64,
        book: i64,
    ) -> Result<bool, DiscountCampaignError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books AS b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                 SELECT 1 FROM book_members AS m
                 WHERE m.book_id=b.id AND m.user_id=$2
                   AND m.role='owner' AND m.is_active
               )
             FOR UPDATE OF b",
        )
        .bind(book)
        .bind(owner)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| DiscountCampaignError::Unavailable)?;
        Ok(found.is_some())
    }

    pub async fn current(
        &self,
        owner: i64,
        book: i64,
    ) -> Result<Option<ActiveAuthorDiscount>, DiscountCampaignError> {
        if owner <= 0 || book <= 0 {
            return Err(DiscountCampaignError::Invalid);
        }
        let result = sqlx::query(
            "SELECT c.id,c.discount_percent,c.min_follow_days,
                    to_char(c.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD') AS ends_on
             FROM author_discount_campaigns AS c
             JOIN books AS b ON b.id=c.book_id AND b.owner_user_id=c.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND b.visibility='public' AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (
                 SELECT 1 FROM book_members AS m
                 WHERE m.book_id=b.id AND m.user_id=$2
                   AND m.role='owner' AND m.is_active
               )
               AND c.status='active' AND c.ends_at > now()",
        )
        .bind(book)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DiscountCampaignError::Unavailable)?;
        result.map_or(Ok(None), |row| {
            Ok(Some(ActiveAuthorDiscount {
                id: row.try_get("id").map_err(|_| DiscountCampaignError::Unavailable)?,
                book_id: book,
                discount_percent: row
                    .try_get("discount_percent")
                    .map_err(|_| DiscountCampaignError::Unavailable)?,
                min_follow_days: row
                    .try_get("min_follow_days")
                    .map_err(|_| DiscountCampaignError::Unavailable)?,
                ends_on: row
                    .try_get("ends_on")
                    .map_err(|_| DiscountCampaignError::Unavailable)?,
            }))
        })
    }

    /// A new campaign cannot change an existing sale or override an active
    /// offer. Stop first and create another; no overlapping discounts.
    pub async fn create(
        &self,
        owner: i64,
        book: i64,
        discount_percent: i16,
        min_follow_days: i16,
        duration_days: i16,
    ) -> Result<i64, DiscountCampaignError> {
        if owner <= 0 || book <= 0
            || !(1..=90).contains(&discount_percent)
            || ![0,90].contains(&min_follow_days)
            || !(1..=90).contains(&duration_days)
        {
            return Err(DiscountCampaignError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| DiscountCampaignError::Unavailable)?;
        if !Self::owned_book(&mut tx, owner, book).await? {
            return Err(DiscountCampaignError::NotFound);
        }
        // Keep campaigns book-scoped; no false offer for a book without paid
        // chapters, or for an account without current B ownership.
        let has_paid: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM chapters AS c
                 JOIN chapter_publications AS p ON p.chapter_id=c.id
                 WHERE c.book_id=$1 AND c.access_level='paid'
                   AND c.visibility='public' AND c.price_amount>0
                   AND NOT c.is_deleted AND NOT c.admin_hold
                   AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             )",
        )
        .bind(book)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DiscountCampaignError::Unavailable)?;
        if !has_paid {
            return Err(DiscountCampaignError::Invalid);
        }
        let inserted = sqlx::query_scalar(
            "INSERT INTO author_discount_campaigns
               (book_id,owner_user_id,discount_percent,min_follow_days,ends_at)
             VALUES($1,$2,$3,$4,clock_timestamp()+make_interval(days=>$5))
             RETURNING id",
        )
        .bind(book)
        .bind(owner)
        .bind(discount_percent)
        .bind(min_follow_days)
        .bind(i32::from(duration_days))
        .fetch_one(&mut *tx)
        .await;
        let id: i64 = inserted.map_err(|error| {
            if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505") {
                DiscountCampaignError::Conflict
            } else {
                DiscountCampaignError::Unavailable
            }
        })?;
        tx.commit().await.map_err(|_| DiscountCampaignError::Unavailable)?;
        Ok(id)
    }

    pub async fn stop(&self, owner: i64, book: i64) -> Result<(), DiscountCampaignError> {
        if owner <= 0 || book <= 0 {
            return Err(DiscountCampaignError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| DiscountCampaignError::Unavailable)?;
        if !Self::owned_book(&mut tx, owner, book).await? {
            return Err(DiscountCampaignError::NotFound);
        }
        sqlx::query(
            "UPDATE author_discount_campaigns SET status='stopped'
             WHERE book_id=$1 AND owner_user_id=$2 AND status='active'",
        )
        .bind(book)
        .bind(owner)
        .execute(&mut *tx)
        .await
        .map_err(|_| DiscountCampaignError::Unavailable)?;
        tx.commit().await.map_err(|_| DiscountCampaignError::Unavailable)?;
        Ok(())
    }
}
