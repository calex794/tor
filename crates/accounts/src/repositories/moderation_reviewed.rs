//! Site-only reviewed decisions index for the three Book moderation queues.
//! Separate from the Book-specific immutable audit and never exposed publicly.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};
use sqlx::Row;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewedCursor {
    pub decided_us: i64,
    pub kind: String,
    pub book_id: i64,
    pub decision_id: i64,
}

impl ReviewedCursor {
    pub fn parse(raw: &str) -> Result<Self, ModerationBooksError> {
        if raw.len() > 96 {
            return Err(ModerationBooksError::Invalid);
        }
        let mut parts = raw.split('.');
        let decided_us = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or(ModerationBooksError::Invalid)?;
        let kind = parts.next().ok_or(ModerationBooksError::Invalid)?;
        let book_id = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or(ModerationBooksError::Invalid)?;
        let decision_id = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or(ModerationBooksError::Invalid)?;
        if parts.next().is_some()
            || decided_us <= 0
            || book_id <= 0
            || decision_id <= 0
            || !matches!(kind, "registration" | "cover" | "deletion")
        {
            return Err(ModerationBooksError::Invalid);
        }
        Ok(Self {
            decided_us,
            kind: kind.to_owned(),
            book_id,
            decision_id,
        })
    }

    pub fn token(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.decided_us, self.kind, self.book_id, self.decision_id
        )
    }
}

#[derive(Debug)]
pub struct ReviewedModerationDecision {
    pub cursor: ReviewedCursor,
    pub book_title: String,
    pub outcome: String,
    pub reviewer_user_id: i64,
    pub reviewer_name: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct ReviewedModerationPage {
    pub items: Vec<ReviewedModerationDecision>,
    pub next_cursor: Option<ReviewedCursor>,
}

impl PgAccounts {
    /// Cross-queue immutable audit, filtered **in PostgreSQL before LIMIT**.
    /// The four-part keyset cursor is stable even when two decisions share
    /// a timestamp or different decisions have the same serial ID.
    pub async fn private_reviewed_moderation_decisions(
        &self,
        session: &[u8; 32],
        kind: Option<&str>,
        outcome: Option<&str>,
        before: Option<&ReviewedCursor>,
    ) -> Result<ReviewedModerationPage, ModerationBooksError> {
        if kind.is_some_and(|k| !matches!(k, "registration" | "cover" | "deletion"))
            || outcome.is_some_and(|o| !matches!(o, "approved" | "rejected"))
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows = sqlx::query(
            "SELECT d.kind,d.book_id,COALESCE(b.title,'Недоступная книга') AS book_title,
                    d.decision_id,d.outcome,d.reviewer_user_id,
                    COALESCE(u.display_name,'Учётная запись удалена') AS reviewer_name,
                    COALESCE(d.reason,'') AS reason,
                    (EXTRACT(EPOCH FROM d.decided_at)*1000000)::bigint AS decided_us
             FROM (
                SELECT 'registration'::text AS kind,d.book_id,d.id AS decision_id,
                       d.outcome::text AS outcome,d.reviewer_user_id,
                       d.reason::text AS reason,d.decided_at
                FROM book_registration_decisions d
                UNION ALL
                SELECT 'cover'::text,d.book_id,d.id,d.outcome::text,
                       d.reviewer_user_id,d.reason::text,d.decided_at
                FROM book_cover_review_decisions d
                UNION ALL
                SELECT 'deletion'::text,d.book_id,d.request_revision,
                       CASE WHEN d.decision='approve' THEN 'approved'
                            ELSE 'rejected' END,
                       d.moderator_user_id,d.reason::text,d.decided_at
                FROM book_ordinary_deletion_decisions d
             ) AS d
             LEFT JOIN books b ON b.id=d.book_id
             LEFT JOIN users u ON u.id=d.reviewer_user_id
             WHERE ($1::text IS NULL OR d.kind=$1)
               AND ($2::text IS NULL OR d.outcome=$2)
               AND ($3::bigint IS NULL OR
                   ((EXTRACT(EPOCH FROM d.decided_at)*1000000)::bigint,
                     d.kind,d.book_id,d.decision_id)
                   < ($3::bigint,$4::text,$5::bigint,$6::bigint))
             ORDER BY decided_us DESC,d.kind DESC,d.book_id DESC,d.decision_id DESC
             LIMIT 41",
        )
        .bind(kind)
        .bind(outcome)
        .bind(before.map(|c| c.decided_us))
        .bind(before.map(|c| c.kind.as_str()))
        .bind(before.map(|c| c.book_id))
        .bind(before.map(|c| c.decision_id))
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|r| {
                Ok(ReviewedModerationDecision {
                    cursor: ReviewedCursor {
                        kind: r
                            .try_get("kind")
                            .map_err(|_| ModerationBooksError::Unavailable)?,
                        book_id: r
                            .try_get("book_id")
                            .map_err(|_| ModerationBooksError::Unavailable)?,
                        decision_id: r
                            .try_get("decision_id")
                            .map_err(|_| ModerationBooksError::Unavailable)?,
                        decided_us: r
                            .try_get("decided_us")
                            .map_err(|_| ModerationBooksError::Unavailable)?,
                    },
                    book_title: r
                        .try_get("book_title")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    outcome: r
                        .try_get("outcome")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    reviewer_user_id: r
                        .try_get("reviewer_user_id")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    reviewer_name: r
                        .try_get("reviewer_name")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    reason: r
                        .try_get("reason")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, ModerationBooksError>>()?;
        let has_more = items.len() > 40;
        items.truncate(40);
        let next_cursor = if has_more {
            items.last().map(|item| item.cursor.clone())
        } else {
            None
        };
        Ok(ReviewedModerationPage { items, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_rejects_truncation_or_parameter_injection() {
        let cursor = ReviewedCursor {
            decided_us: 1800000000000000,
            kind: "cover".to_owned(),
            book_id: 20,
            decision_id: 37,
        };
        assert_eq!(ReviewedCursor::parse(&cursor.token()).unwrap(), cursor);
        for invalid in [
            "", "1", "0.cover.1.1", "1.pending.1.1", "1.cover.0.1",
            "1.cover.1.0", "1.cover.1.1.extra", "1.cover.1.1%20",
            "999999999999999999999.cover.1.1",
        ] {
            assert!(matches!(
                ReviewedCursor::parse(invalid),
                Err(ModerationBooksError::Invalid)
            ));
        }
    }
}
