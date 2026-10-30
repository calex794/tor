//! G-07/G-08: one bounded, explicitly invoked outbox processing pass.
//! No autonomous daemon, external email/push delivery or authorization grants.
use crate::repositories::{
    notify::PgNotificationRecipients,
    outbox::{FailureDisposition, OutboxError, PgOutbox},
};
use crate::services::notify::RecipientError;

const EVENT_TYPE: &str = "ChapterPublished";
const CONSUMER: &str = "subscription-inbox-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationRun {
    Idle,
    Staged { recipients: u64, pages: u64 },
    RetryScheduled,
    DeadLetter,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationWorkerError {
    Queue(OutboxError),
}

pub struct NotificationWorker {
    outbox: PgOutbox,
    recipients: PgNotificationRecipients,
}

impl NotificationWorker {
    pub fn new(outbox: PgOutbox, recipients: PgNotificationRecipients) -> Self {
        Self { outbox, recipients }
    }

    /// A run owns one event lease and renews it every completed 100-reader
    /// transaction. A restarted process will resume at the PostgreSQL cursor.
    /// Only the subscriber consumer is acknowledged. The independent search
    /// consumer remains pending until its own projection is committed.
    pub async fn run_one(&self) -> Result<NotificationRun, NotificationWorkerError> {
        let Some(event) = self
            .outbox
            .claim_next_for_consumer(EVENT_TYPE, CONSUMER)
            .await
            .map_err(NotificationWorkerError::Queue)?
        else {
            return Ok(NotificationRun::Idle);
        };
        let mut recipients = 0_u64;
        let mut pages = 0_u64;
        let mut previous_cursor = 0_i64;
        loop {
            let page = self
                .recipients
                .stage_next(&event.id, event.lease_epoch)
                .await;
            match page {
                Ok(batch) => {
                    recipients = recipients.saturating_add(batch.newly_staged);
                    pages = pages.saturating_add(1);
                    if batch.complete {
                        return Ok(NotificationRun::Staged { recipients, pages });
                    }
                    if batch.cursor_user_id <= previous_cursor {
                        return self
                            .record_problem(&event.id, event.lease_epoch, false)
                            .await;
                    }
                    previous_cursor = batch.cursor_user_id;
                    tokio::task::yield_now().await;
                }
                Err(RecipientError::LostLease) => {
                    return Ok(NotificationRun::Superseded);
                }
                Err(error) => {
                    return self
                        .record_problem(
                            &event.id,
                            event.lease_epoch,
                            matches!(error, RecipientError::Unavailable),
                        )
                        .await;
                }
            }
        }
    }

    async fn record_problem(
        &self,
        event_id: &str,
        epoch: i64,
        retryable: bool,
    ) -> Result<NotificationRun, NotificationWorkerError> {
        match self
            .outbox
            .record_failure(event_id, epoch, retryable)
            .await
        {
            Ok(FailureDisposition::RetryScheduled) => Ok(NotificationRun::RetryScheduled),
            Ok(FailureDisposition::DeadLetter) => Ok(NotificationRun::DeadLetter),
            Err(OutboxError::LostLease) => Ok(NotificationRun::Superseded),
            Err(error) => Err(NotificationWorkerError::Queue(error)),
        }
    }
}
