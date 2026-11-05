//! MOD-COVER-01: the only site-moderator authority to publish a processed cover.
//! This is distinct from G's worker and from the Book member "moderator" role.
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgCoverReview {
    pool: PgPool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CoverReviewError {
    Denied,
    Invalid,
    Missing,
    Stale,
    Unavailable,
}

#[derive(Debug)]
pub struct PendingCoverReview {
    pub id: i64,
    pub book_id: i64,
    pub title: String,
    pub job_id: i64,
    pub submitted_by_user_id: i64,
    pub expected_slot_revision: i64,
}

#[derive(Debug)]
pub struct PrivateCoverPreview {
    pub storage_key: String,
    pub sha256: [u8; 32],
    pub size_bytes: i64,
}

#[derive(Debug)]
pub struct CoverDecisionHistory {
    pub request_id: i64,
    pub job_id: i64,
    pub reviewer_user_id: i64,
    pub outcome: String,
    pub reason: Option<String>,
    pub decided_unix: i64,
}

async fn live_site_moderator(
    tx: &mut Transaction<'_, Postgres>,
    session: &[u8; 32],
) -> Result<i64, CoverReviewError> {
    let actor: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM users u
         JOIN sessions s ON s.user_id=u.id AND s.token_hash=$1
           AND s.revoked_at IS NULL AND s.expires_at>now()
         JOIN account_roles role ON role.user_id=u.id
           AND role.role IN ('moderator','administrator') AND role.is_active
         WHERE u.account_state='active'
         ORDER BY role.role LIMIT 1
         FOR SHARE OF u,s,role",
    )
    .bind(session.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CoverReviewError::Unavailable)?;
    actor.ok_or(CoverReviewError::Denied)
}

impl PgCoverReview {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only a current site-role holder can list submitted private metadata.
    pub async fn pending(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<PendingCoverReview>, CoverReviewError> {
        if before.is_some_and(|value| value <= 0) {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        live_site_moderator(&mut tx, session).await?;
        let rows = sqlx::query(
            "SELECT r.id,r.book_id,r.job_id,r.submitted_by_user_id,
                    r.expected_slot_revision,b.title
             FROM book_cover_review_requests r
             JOIN books b ON b.id=r.book_id
             WHERE r.state='pending' AND ($1::BIGINT IS NULL OR r.id<$1)
             ORDER BY r.id DESC LIMIT 40",
        )
        .bind(before)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        rows.into_iter()
            .map(|r| {
                Ok(PendingCoverReview {
                    id: r.try_get("id").map_err(|_| CoverReviewError::Unavailable)?,
                    book_id: r
                        .try_get("book_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    title: r
                        .try_get("title")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    job_id: r
                        .try_get("job_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    submitted_by_user_id: r
                        .try_get("submitted_by_user_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    expected_slot_revision: r
                        .try_get("expected_slot_revision")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                })
            })
            .collect()
    }

    pub async fn pending_count(&self, session: &[u8; 32]) -> Result<i64, CoverReviewError> {
        let mut tx = self.pool.begin().await.map_err(|_| CoverReviewError::Unavailable)?;
        live_site_moderator(&mut tx, session).await?;
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_cover_review_requests WHERE state='pending'",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        Ok(count)
    }

    /// Never put these internal S3 keys into an HTML page or public JSON.
    /// The HTTP preview route proxies verified WebP bytes with no-store.
    pub async fn preview(
        &self,
        session: &[u8; 32],
        request_id: i64,
    ) -> Result<Option<PrivateCoverPreview>, CoverReviewError> {
        if request_id <= 0 {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        live_site_moderator(&mut tx, session).await?;
        let row = sqlx::query(
            "SELECT v.storage_key,v.content_sha256,v.size_bytes
             FROM book_cover_review_requests r
             JOIN cover_variants v ON v.job_id=r.job_id AND v.variant_name='main'
             WHERE r.id=$1 AND r.state='pending'",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        row.map(|r| {
            let hash: Vec<u8> = r
                .try_get("content_sha256")
                .map_err(|_| CoverReviewError::Unavailable)?;
            let hash: [u8; 32] = hash.try_into().map_err(|_| CoverReviewError::Unavailable)?;
            let storage_key: String = r
                .try_get("storage_key")
                .map_err(|_| CoverReviewError::Unavailable)?;
            let size_bytes: i64 = r
                .try_get("size_bytes")
                .map_err(|_| CoverReviewError::Unavailable)?;
            if !storage_key.starts_with("staging/covers/variants/")
                || storage_key.contains("..")
                || size_bytes <= 0
                || size_bytes > 8 * 1024 * 1024
            {
                return Err(CoverReviewError::Unavailable);
            }
            Ok(PrivateCoverPreview {
                storage_key,
                sha256: hash,
                size_bytes,
            })
        })
        .transpose()
    }

    /// The author may neither approve own Book nor bypass the decision by a
    /// direct Book slot write. Slot, reviewer audit and request are one commit.
    pub async fn decide(
        &self,
        session: &[u8; 32],
        request_id: i64,
        expected_slot_revision: i64,
        approve: bool,
        reason: &str,
    ) -> Result<(), CoverReviewError> {
        let reason = reason.trim();
        if request_id <= 0
            || expected_slot_revision < 0
            || reason.len() > 2000
            || (!approve && reason.is_empty())
        {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        let moderator = live_site_moderator(&mut tx, session).await?;
        let row = sqlx::query(
            "SELECT r.book_id,r.job_id,r.state,r.expected_slot_revision,
                    b.owner_user_id,b.admin_hold,b.is_deleted,
                    b.lifecycle_status,b.visibility_status,
                    j.status AS job_status,j.actor_user_id
             FROM book_cover_review_requests r
             JOIN books b ON b.id=r.book_id
             JOIN cover_variant_jobs j ON j.id=r.job_id
             WHERE r.id=$1 FOR UPDATE OF r,b,j",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?
        .ok_or(CoverReviewError::Missing)?;
        let book_id: i64 = row
            .try_get("book_id")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let job_id: i64 = row
            .try_get("job_id")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let owner: i64 = row
            .try_get("owner_user_id")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let actor: i64 = row
            .try_get("actor_user_id")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let status: String = row
            .try_get("state")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let job_status: String = row
            .try_get("job_status")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let revision: i64 = row
            .try_get("expected_slot_revision")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let lifecycle: String = row
            .try_get("lifecycle_status")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let visibility: String = row
            .try_get("visibility_status")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let held: bool = row
            .try_get("admin_hold")
            .map_err(|_| CoverReviewError::Unavailable)?;
        let deleted: bool = row
            .try_get("is_deleted")
            .map_err(|_| CoverReviewError::Unavailable)?;
        if status != "pending" || revision != expected_slot_revision {
            return Err(CoverReviewError::Stale);
        }
        if moderator == owner || moderator == actor {
            return Err(CoverReviewError::Denied);
        }
        if held
            || deleted
            || owner != actor
            || matches!(lifecycle.as_str(), "pending_deletion" | "ordinary_deleted")
            || matches!(visibility.as_str(), "blocked" | "hidden")
        {
            return Err(CoverReviewError::Stale);
        }
        if approve {
            if job_status != "done" {
                return Err(CoverReviewError::Stale);
            }
            let pair_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM cover_variants v WHERE v.job_id=$1
                   AND v.variant_name IN ('main','card') AND v.size_bytes>0",
            )
            .bind(job_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
            if pair_count != 2 {
                return Err(CoverReviewError::Stale);
            }
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM book_cover_slots WHERE book_id=$1 FOR UPDATE",
            )
            .bind(book_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
            if current.unwrap_or(0) != revision {
                return Err(CoverReviewError::Stale);
            }
        }
        let new_state = if approve { "approved" } else { "rejected" };
        sqlx::query(
            "UPDATE book_cover_review_requests
             SET state=$2,reviewer_user_id=$3,decided_at=now(),reason=$4
             WHERE id=$1 AND state='pending'",
        )
        .bind(request_id)
        .bind(new_state)
        .bind(moderator)
        .bind(if reason.is_empty() {
            None
        } else {
            Some(reason)
        })
        .execute(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        sqlx::query(
            "INSERT INTO book_cover_review_decisions
             (request_id,book_id,job_id,reviewer_user_id,outcome,reason)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(request_id)
        .bind(book_id)
        .bind(job_id)
        .bind(moderator)
        .bind(new_state)
        .bind(if reason.is_empty() {
            None
        } else {
            Some(reason)
        })
        .execute(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        if approve {
            let changed: Option<i64> = sqlx::query_scalar(
                "INSERT INTO book_cover_slots(book_id,job_id,revision)
                 VALUES($1,$2,1)
                 ON CONFLICT(book_id) DO UPDATE
                   SET job_id=EXCLUDED.job_id,
                       revision=book_cover_slots.revision+1,attached_at=now()
                   WHERE book_cover_slots.revision=$3
                 RETURNING revision",
            )
            .bind(book_id)
            .bind(job_id)
            .bind(revision)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
            if changed.is_none() {
                return Err(CoverReviewError::Stale);
            }
        }
        tx.commit()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        Ok(())
    }

    /// Reviewer attribution is NEVER returned by the public book APIs.
    pub async fn private_history(
        &self,
        session: &[u8; 32],
        book_id: i64,
    ) -> Result<Vec<CoverDecisionHistory>, CoverReviewError> {
        if book_id <= 0 {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        live_site_moderator(&mut tx, session).await?;
        let rows = sqlx::query(
            "SELECT d.request_id,d.job_id,d.reviewer_user_id,d.outcome,d.reason,
                    EXTRACT(EPOCH FROM d.decided_at)::bigint AS decided_unix
             FROM book_cover_review_decisions d WHERE d.book_id=$1
             ORDER BY d.id DESC LIMIT 100",
        )
        .bind(book_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        rows.into_iter()
            .map(|r| {
                Ok(CoverDecisionHistory {
                    request_id: r
                        .try_get("request_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    job_id: r
                        .try_get("job_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    reviewer_user_id: r
                        .try_get("reviewer_user_id")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    outcome: r
                        .try_get("outcome")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    reason: r
                        .try_get("reason")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                    decided_unix: r
                        .try_get("decided_unix")
                        .map_err(|_| CoverReviewError::Unavailable)?,
                })
            })
            .collect()
    }
}
