//! Stable B-owned creation command hashes.
//! Existing command keys preserve their hashes if taxonomy is empty.
use crate::services::authoring::NewBook;
use sha2::{Digest, Sha256};

pub(super) fn payload_hash(book: &NewBook) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in [&book.source_identity, &book.target_language, &book.title] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    // Keep old hashes for exact retries created without optional metadata.
    if !book.author_name.is_empty() || !book.genres.is_empty() {
        let genres = book.genres.join("\n");
        for part in [book.author_name.as_str(), genres.as_str()] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
    }
    // Preserve existing B-01 command digests when taxonomy is absent.
    if !book.tags.is_empty() || !book.fandoms.is_empty() {
        for part in [book.tags.join("\n"), book.fandoms.join("\n")] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
    }
    let digest: [u8; 32] = hasher.finalize().into();

    digest
}
