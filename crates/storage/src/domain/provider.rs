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
