//! G/S3-04: fail-closed object provider errors and bounded ciphertext sizes.
use tor_contracts::paid_delivery::ObjectRef;

pub const MAX_PRIVATE_CIPHERTEXT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageError {
    Invalid,
    Conflict,
    Corrupt,
    Unavailable,
}

pub fn valid_ciphertext_size(object: &ObjectRef) -> bool {
    (17..=MAX_PRIVATE_CIPHERTEXT_BYTES).contains(&object.size_bytes)
}

/// Match the fixed private DNS-compatible bucket policy used by G's S3 port.
/// Bucket values come from trusted configuration, never browser requests.
pub fn valid_private_bucket(bucket: &str) -> bool {
    let bytes = bucket.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !bucket.contains("--")
        && !bucket.starts_with("public")
}
