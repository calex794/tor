//! Reauthorize the exact edition after the potentially slow S3 signing request.
//! No bearer capability is returned after a hide, revocation or version change.
use axum::http::StatusCode;
use std::future::Future;
use tor_contracts::paid_delivery::SecretValue;
use tor_reading::repositories::free_s3_ref::FreeS3Ref;

pub(super) async fn issue<R, S, Lookup, Sign>(
    mut resolve: R,
    sign: S,
) -> Result<(FreeS3Ref, SecretValue), StatusCode>
where
    R: FnMut() -> Lookup,
    Lookup: Future<Output = Result<Option<FreeS3Ref>, StatusCode>>,
    S: FnOnce(FreeS3Ref) -> Sign,
    Sign: Future<Output = Result<SecretValue, StatusCode>>,
{
    let initial = resolve().await?.ok_or(StatusCode::NOT_FOUND)?;
    let url = sign(initial.clone()).await?;
    match resolve().await? {
        None => Err(StatusCode::NOT_FOUND),
        Some(current) if current != initial => Err(StatusCode::CONFLICT),
        Some(_) => Ok((initial, url)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque, future::ready};

    fn edition() -> FreeS3Ref {
        FreeS3Ref {
            chapter_id: 101,
            version_id: 201,
            sha256: [17; 32],
            size_bytes: 42,
            bucket: "tor-free-ci".to_owned(),
        }
    }

    fn signed() -> SecretValue {
        SecretValue::new("https://s3.example.invalid/private?X-Amz-Signature=test".into()).unwrap()
    }

    #[tokio::test]
    async fn denies_before_signing_when_initial_authority_is_absent_or_unavailable() {
        for result in [Ok(None), Err(StatusCode::SERVICE_UNAVAILABLE)] {
            let expected = if result.is_ok() {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            let calls = Cell::new(0);
            let outcome = issue(
                || ready(result.clone()),
                |_| {
                    calls.set(calls.get() + 1);
                    ready(Ok(signed()))
                },
            )
            .await;
            assert_eq!(outcome.err(), Some(expected));
            assert_eq!(calls.get(), 0, "never consult S3 for a denied chapter");
        }
    }

    #[tokio::test]
    async fn repeats_authority_check_after_signing_before_releasing_url() {
        let lookups = Cell::new(0);
        let signs = Cell::new(0);
        let (record, url) = issue(
            || {
                let count = lookups.get();
                assert_eq!(signs.get(), usize::from(count > 0));
                lookups.set(count + 1);
                ready(Ok(Some(edition())))
            },
            |record| {
                assert_eq!(lookups.get(), 1);
                assert_eq!(record, edition());
                signs.set(1);
                ready(Ok(signed()))
            },
        )
        .await
        .unwrap();
        assert_eq!(record, edition());
        assert_eq!(url, signed());
        assert_eq!(lookups.get(), 2);
        assert_eq!(signs.get(), 1);
        assert!(!format!("{url:?}").contains("X-Amz-Signature"));
    }

    #[tokio::test]
    async fn hides_or_age_revocations_during_signing_never_return_the_url() {
        let mut states = VecDeque::from([Ok(Some(edition())), Ok(None)]);
        let outcome = issue(
            || ready(states.pop_front().expect("exactly two authorization reads")),
            |_| ready(Ok(signed())),
        )
        .await;
        assert_eq!(outcome.err(), Some(StatusCode::NOT_FOUND));
        assert!(states.is_empty());
    }

    #[tokio::test]
    async fn unavailable_recheck_is_not_misclassified_as_an_authorized_grant() {
        let mut states =
            VecDeque::from([Ok(Some(edition())), Err(StatusCode::SERVICE_UNAVAILABLE)]);
        let outcome = issue(
            || ready(states.pop_front().unwrap()),
            |_| ready(Ok(signed())),
        )
        .await;
        assert_eq!(outcome.err(), Some(StatusCode::SERVICE_UNAVAILABLE));
    }

    #[tokio::test]
    async fn changed_edition_digest_size_bucket_or_chapter_rejects_stale_url() {
        for index in 0..5 {
            let mut changed = edition();
            match index {
                0 => changed.version_id += 1,
                1 => changed.sha256 = [18; 32],
                2 => changed.size_bytes += 1,
                3 => changed.bucket = "another-private-bucket".to_owned(),
                _ => changed.chapter_id += 1,
            }
            let mut states = VecDeque::from([Ok(Some(edition())), Ok(Some(changed))]);
            let outcome = issue(
                || ready(states.pop_front().unwrap()),
                |_| ready(Ok(signed())),
            )
            .await;
            assert_eq!(outcome.err(), Some(StatusCode::CONFLICT));
        }
    }

    #[tokio::test]
    async fn storage_failure_never_releases_a_grant() {
        let lookups = Cell::new(0);
        let outcome = issue(
            || {
                lookups.set(lookups.get() + 1);
                ready(Ok(Some(edition())))
            },
            |_| ready(Err(StatusCode::SERVICE_UNAVAILABLE)),
        )
        .await;
        assert_eq!(outcome.err(), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(lookups.get(), 1);
    }
}
