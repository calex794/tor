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
    // Empty descriptions preserve legacy idempotency hashes.
    if !book.description.is_empty() {
        hasher.update(b"book-description-v1");
        hasher.update((book.description.len() as u64).to_be_bytes());
        hasher.update(book.description.as_bytes());
    }
    if !book.original_title.is_empty()
        || !book.original_language.is_empty()
        || !book.source_url.is_empty()
        || book.release_year.is_some()
        || !book.project_status.is_empty()
        || !book.catalog_section.is_empty()
        || !book.alt_titles.is_empty()
    {
        hasher.update(b"book-registration-v1");
        for part in [
            book.original_title.as_str(),
            book.original_language.as_str(),
            book.source_url.as_str(),
            book.project_status.as_str(),
            book.catalog_section.as_str(),
        ] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hasher.update(book.release_year.unwrap_or(0).to_be_bytes());
        for part in &book.alt_titles {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
    }
    let digest: [u8; 32] = hasher.finalize().into();

    digest
}
