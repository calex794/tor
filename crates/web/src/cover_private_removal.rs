//! Common completion mechanics for explicitly invoked private cover cleanup.
//! Eligibility, object selection, capability and lease ownership remain with each caller.
use std::{future::Future, time::Duration};
use tor_ingest::domain::cover::CoverError;

pub(super) const PRIVATE_REMOVAL_TIMEOUT: Duration = Duration::from_secs(12);

/// Poll exactly one fenced ledger update after the bounded storage operation.
/// A timeout or storage error schedules retry, never successful removal.
/// Return true only after a successful removal has been acknowledged by the ledger.
pub(super) async fn finish_private_removal<E>(
    timeout: Duration,
    operation: impl Future<Output = Result<(), CoverError>>,
    mark_removed: impl Future<Output = Result<bool, E>>,
    mark_retry: impl Future<Output = Result<bool, E>>,
) -> Result<bool, CoverError> {
    let removed = matches!(tokio::time::timeout(timeout, operation).await, Ok(Ok(())));
    let updated = if removed {
        mark_removed.await
    } else {
        mark_retry.await
    };
    if !updated.map_err(|_| CoverError::Unavailable)? {
        return Err(CoverError::LostLease);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        future::{pending, ready},
    };

    #[tokio::test]
    async fn success_marks_only_removed_after_storage() {
        let storage_done = Cell::new(false);
        let result = finish_private_removal(
            PRIVATE_REMOVAL_TIMEOUT,
            async {
                storage_done.set(true);
                Ok(())
            },
            async {
                assert!(storage_done.get());
                Ok::<_, ()>(true)
            },
            async { panic!("successful removal must not retry") },
        )
        .await;
        assert_eq!(result, Ok(true));
    }

    #[tokio::test]
    async fn storage_failure_marks_only_retry() {
        let result = finish_private_removal(
            PRIVATE_REMOVAL_TIMEOUT,
            ready(Err(CoverError::Forbidden)),
            async { panic!("failed removal must not be marked removed") },
            ready(Ok::<_, ()>(true)),
        )
        .await;
        assert_eq!(result, Ok(false));
    }

    #[tokio::test]
    async fn timeout_marks_only_retry() {
        let result = finish_private_removal(
            Duration::ZERO,
            pending(),
            async { panic!("timed out removal must not be marked removed") },
            ready(Ok::<_, ()>(true)),
        )
        .await;
        assert_eq!(result, Ok(false));
    }

    #[tokio::test]
    async fn lost_lease_and_ledger_error_are_fail_closed_on_both_paths() {
        for (update, expected) in [
            (Ok(false), Err(CoverError::LostLease)),
            (Err(()), Err(CoverError::Unavailable)),
        ] {
            assert_eq!(
                finish_private_removal(
                    PRIVATE_REMOVAL_TIMEOUT,
                    ready(Ok(())),
                    ready(update),
                    async { panic!("no fallback ledger update after failure") },
                )
                .await,
                expected
            );
            assert_eq!(
                finish_private_removal(
                    PRIVATE_REMOVAL_TIMEOUT,
                    ready(Err(CoverError::Unavailable)),
                    async { panic!("failed storage operation cannot be acknowledged") },
                    ready(update),
                )
                .await,
                expected
            );
        }
    }
}
