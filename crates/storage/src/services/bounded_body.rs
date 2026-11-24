//! Shared bounded response-body read for trusted S3 locators.
//! Not a proof of identity, size, checksum, encoding or permission.
use crate::domain::provider::StorageError;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Read at most expected_bytes + 1, including an oversize sentinel byte.
/// The caller must validate its own size limit before issuing a GET and
/// classify short/long bodies, metadata, hash and encoding afterward.
/// Never use a provider-supplied Content-Length as the allocation bound.
pub(crate) async fn read_bounded_body(
    reader: impl AsyncRead + Unpin,
    expected_bytes: u64,
) -> Result<Vec<u8>, StorageError> {
    let limit = expected_bytes.checked_add(1).ok_or(StorageError::Invalid)?;
    let mut reader = reader.take(limit);
    let mut bytes = Vec::with_capacity(expected_bytes.min(65_536) as usize);
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| StorageError::Unavailable)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    #[tokio::test]
    async fn exact_and_short_body_return_observed_bytes() {
        assert_eq!(read_bounded_body(&b"abc"[..], 3).await.unwrap(), b"abc");
        assert_eq!(read_bounded_body(&b"ab"[..], 3).await.unwrap(), b"ab");
        assert!(read_bounded_body(&b""[..], 3).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_body_reads_only_one_extra_byte() {
        let mut source = &b"abcdef"[..];
        let observed = read_bounded_body(&mut source, 3).await.unwrap();
        assert_eq!(observed, b"abcd");
        assert_eq!(source, b"ef");
    }

    #[tokio::test]
    async fn overflowing_bound_is_rejected_without_reading() {
        let mut source = &b"abc"[..];
        assert_eq!(
            read_bounded_body(&mut source, u64::MAX).await,
            Err(StorageError::Invalid)
        );
        assert_eq!(source, b"abc");
    }

    struct FailedRead;

    impl AsyncRead for FailedRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("test transport failure")))
        }
    }

    #[tokio::test]
    async fn read_error_is_unavailable_not_corruption() {
        assert_eq!(
            read_bounded_body(FailedRead, 3).await,
            Err(StorageError::Unavailable)
        );
    }
}
