//! G-07 / S066: one audited, administrator-only dead-letter requeue.
//! Never dispatch here, delete receipts or bypass the shared outbox.
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminRetryError {
    InvalidId,
    Forbidden,
    NotFound,
    NotRetryable,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRetryReceipt {
    pub event_id: String,
    pub previous_attempts: i32,
    pub lease_epoch: i64,
}

#[derive(Clone)]
pub struct PgOutboxAdminRetry {
    pool: PgPool,
}

fn valid_event_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, value)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *value == b'-'
            } else {
                value.is_ascii_hexdigit()
            }
        })
}

/// Only in-app projection handlers reviewed for deduplicated replay.
/// Do not requeue financial, external-delivery or unknown job families.
fn supported_retry_event(event_type: &str, consumers: &[String]) -> bool {
    if consumers.len() != 1 {
        return false;
    }
    matches!(
        (event_type, consumers[0].as_str()),
        ("ChapterPublished", "chapter_release_fanout_v1")
            | ("reader.chapter.released", "user_notifications")
    )
}

impl PgOutboxAdminRetry {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The active session, account and administrator role are locked in the
    /// SAME transaction as the event and append-only audit write. Role or
    /// session revocation cannot race a successful privileged requeue.
    pub async fn retry_dead_letter(
        &self,
        session_digest: &[u8; 32],
        event_id: &str,
    ) -> Result<AdminRetryReceipt, AdminRetryError> {
        if !valid_event_id(event_id) {
            return Err(AdminRetryError::InvalidId);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AdminRetryError::Unavailable)?;
        let admin: Option<i64> = sqlx::query_scalar(
            "SELECT s.user_id
             FROM sessions s
             JOIN users u ON u.id=s.user_id
             JOIN account_roles r ON r.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>clock_timestamp()
               AND u.account_state='active'
               AND r.role='administrator' AND r.is_active
             FOR SHARE OF s,u,r",
        )
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AdminRetryError::Unavailable)?;
        let Some(admin) = admin else {
            tx.rollback()
                .await
                .map_err(|_| AdminRetryError::Unavailable)?;
            return Err(AdminRetryError::Forbidden);
        };

        // A row lock serializes manual retries with worker acknowledgements.
        // An active lease is never overridden; an expired one is fenced by
        // incrementing lease_epoch before making the event due again.
        let event: Option<(String, Vec<String>, bool, bool, bool, i32, i64)> = sqlx::query_as(
            "SELECT event_type,required_consumers,
                    delivered_at IS NOT NULL,
                    dead_at IS NOT NULL,
                    COALESCE(lease_until>clock_timestamp(),false),
                    attempts,lease_epoch
             FROM outbox_events
             WHERE id=$1::uuid
             FOR UPDATE",
        )
        .bind(event_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AdminRetryError::Unavailable)?;
        let Some((
            event_type,
            consumers,
            delivered,
            dead,
            active_lease,
            previous_attempts,
            previous_epoch,
        )) = event
        else {
            tx.rollback()
                .await
                .map_err(|_| AdminRetryError::Unavailable)?;
            return Err(AdminRetryError::NotFound);
        };
        if !supported_retry_event(&event_type, &consumers) || delivered || !dead || active_lease {
            // A rejected operator action must release the administrator role
            // SHARE lock before returning an HTTP response. SQLx rollback on
            // Drop is asynchronous; callers may immediately revoke that role.
            tx.rollback()
                .await
                .map_err(|_| AdminRetryError::Unavailable)?;
            return Err(AdminRetryError::NotRetryable);
        }

        let Some(new_epoch) = previous_epoch.checked_add(1) else {
            tx.rollback()
                .await
                .map_err(|_| AdminRetryError::Unavailable)?;
            return Err(AdminRetryError::NotRetryable);
        };
        sqlx::query(
            "UPDATE outbox_events
             SET dead_at=NULL, attempts=0, next_attempt_at=clock_timestamp(),
                 lease_until=NULL, lease_epoch=lease_epoch+1
             WHERE id=$1::uuid",
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AdminRetryError::Unavailable)?;
        // A failed audit insert rolls the requeue back, rather than silently
        // creating an unaccountable operator action.
        sqlx::query(
            "INSERT INTO g_outbox_admin_retry_audit(
                 admin_user_id,event_id,previous_attempts,
                 previous_lease_epoch,resumed_lease_epoch)
             VALUES($1,$2::uuid,$3,$4,$5)",
        )
        .bind(admin)
        .bind(event_id)
        .bind(previous_attempts)
        .bind(previous_epoch)
        .bind(new_epoch)
        .execute(&mut *tx)
        .await
        .map_err(|_| AdminRetryError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| AdminRetryError::Unavailable)?;

        Ok(AdminRetryReceipt {
            event_id: event_id.to_owned(),
            previous_attempts,
            lease_epoch: new_epoch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{supported_retry_event, valid_event_id};

    #[test]
    fn g_07_admin_retry_only_accepts_reviewed_internal_consumers() {
        assert!(supported_retry_event(
            "ChapterPublished",
            &["chapter_release_fanout_v1".to_string()]
        ));
        assert!(supported_retry_event(
            "reader.chapter.released",
            &["user_notifications".to_string()]
        ));
        for (kind, consumer) in [
            ("payment.capture", "user_notifications"),
            ("ChapterPublished", "external_mail"),
            ("reader.chapter.released", "external_mail"),
        ] {
            assert!(!supported_retry_event(kind, &[consumer.to_string()]));
        }
        assert!(!supported_retry_event(
            "ChapterPublished",
            &["chapter_release_fanout_v1".to_string(), "external_mail".to_string()]
        ));
    }

    #[test]
    fn g_07_retry_requires_one_canonical_uuid_path_component() {
        assert!(valid_event_id("00000000-0000-0000-0000-000000000000"));
        assert!(valid_event_id("ABCDEF01-2345-6789-ABCD-0123456789ab"));
        for id in [
            "",
            "00000000000000000000000000000000",
            "00000000-0000-0000-0000-00000000000Z",
            "00000000-0000-0000-0000-000000000000/next",
        ] {
            assert!(!valid_event_id(id));
        }
    }
}
