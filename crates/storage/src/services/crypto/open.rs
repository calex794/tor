//! G-only authenticated opener for a registered encrypted paid chapter.
//!
//! Only a server-owned reader may call this after PostgreSQL has checked
//! author ownership and the exact B edition/object membership. The browser
//! must never supply a bucket, object ID, AAD or wrapping material. This
//! primitive does NOT grant paid reading and does not expose HTTP routes.
use super::{
    chapter_aad, decrypt_raw, sha256_bytes, unwrap_registered_dek_from_keyring, ContentKeyring,
    CryptoError, MAX_CHAPTER_CHARS, MAX_CHAPTER_UTF8_BYTES,
};
use flate2::read::GzDecoder;
use std::io::Read;
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId};
use zeroize::Zeroizing;

/// Metadata read from the trusted, attached G registry, not user input.
pub(crate) struct RegisteredPaidChapter<'a> {
    pub version: ChapterVersionRef,
    pub object_id: &'a ObjectId,
    pub ciphertext_len: u64,
    pub ciphertext_sha256: &'a [u8; 32],
    pub plaintext_sha256: &'a [u8; 32],
    pub object_nonce: &'a [u8; 12],
    pub aad: &'a [u8],
    pub wrapped_dek: &'a [u8],
    pub wrap_nonce: &'a [u8; 12],
    pub kek_id: &'a str,
}

/// Authenticates the downloaded ciphertext, unwraps only the registered
/// version's KEK, and inflates into a strictly bounded zeroizing buffer.
/// No unverified bytes are presented to the author editor.
pub(crate) fn open_registered_paid_chapter(
    registry: RegisteredPaidChapter<'_>,
    ciphertext: &[u8],
    keyring: &ContentKeyring,
) -> Result<Zeroizing<String>, CryptoError> {
    if ciphertext.len() < 17
        || ciphertext.len() as u64 != registry.ciphertext_len
        || sha256_bytes(ciphertext) != *registry.ciphertext_sha256
        || registry.aad
            != chapter_aad(
                registry.version,
                registry.object_id,
                registry.plaintext_sha256,
            )
    {
        return Err(CryptoError::AuthenticationFailed);
    }
    let dek = unwrap_registered_dek_from_keyring(
        registry.object_id,
        registry.wrapped_dek,
        registry.wrap_nonce,
        registry.kek_id,
        keyring,
    )?;
    let compressed = Zeroizing::new(decrypt_raw(
        &dek,
        registry.object_nonce,
        registry.aad,
        ciphertext,
    )?);
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut plain = Zeroizing::new(Vec::new());
    decoder
        .by_ref()
        .take(MAX_CHAPTER_UTF8_BYTES as u64 + 1)
        .read_to_end(&mut plain)
        .map_err(|_| CryptoError::CompressionFailed)?;
    if plain.is_empty()
        || plain.len() > MAX_CHAPTER_UTF8_BYTES
        || sha256_bytes(&plain) != *registry.plaintext_sha256
    {
        return Err(CryptoError::AuthenticationFailed);
    }
    let body = std::str::from_utf8(&plain).map_err(|_| CryptoError::InvalidMaterial)?;
    if body.chars().count() > MAX_CHAPTER_CHARS || body.contains('\0') {
        return Err(CryptoError::InvalidMaterial);
    }
    Ok(Zeroizing::new(body.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::crypto::{seal_chapter_with_keyring, ContentKek, EncryptedChapter};
    use std::sync::Arc;
    use tor_contracts::paid_delivery::PositiveId;

    fn version() -> ChapterVersionRef {
        ChapterVersionRef {
            chapter_id: PositiveId::new(321).unwrap(),
            version_id: PositiveId::new(432).unwrap(),
        }
    }

    fn metadata(sealed: &EncryptedChapter) -> RegisteredPaidChapter<'_> {
        RegisteredPaidChapter {
            version: sealed.version,
            object_id: &sealed.object_id,
            ciphertext_len: sealed.ciphertext.len() as u64,
            ciphertext_sha256: &sealed.ciphertext_sha256,
            plaintext_sha256: &sealed.plaintext_sha256,
            object_nonce: &sealed.object_nonce,
            aad: &sealed.aad,
            wrapped_dek: &sealed.wrapped_dek,
            wrap_nonce: &sealed.wrap_nonce,
            kek_id: sealed.kek_id,
        }
    }

    #[test]
    fn registered_paid_markdown_roundtrips_only_authenticated_bytes() {
        let ring = ContentKeyring::v1_only(Arc::new(ContentKek::from_bytes([7; 32])));
        let id = ObjectId::parse("c0a00000-0001-4000-8000-000000000001").unwrap();
        let source = "# Платная V1\nСтрока 😀";
        let sealed = seal_chapter_with_keyring(version(), id, source, &ring).unwrap();
        let opened =
            open_registered_paid_chapter(metadata(&sealed), &sealed.ciphertext, &ring).unwrap();
        assert_eq!(opened.as_str(), source);

        let mut tampered = sealed.ciphertext.clone();
        tampered[0] ^= 0x80;
        assert!(
            open_registered_paid_chapter(metadata(&sealed), &tampered, &ring).is_err(),
            "ciphertext digest mismatch must fail closed"
        );
        let mut metadata = metadata(&sealed);
        let incorrect_sha = [11; 32];
        metadata.plaintext_sha256 = &incorrect_sha;
        assert!(
            open_registered_paid_chapter(metadata, &sealed.ciphertext, &ring).is_err(),
            "version-bound AAD cannot be substituted"
        );
    }

    #[test]
    fn wrong_kek_and_version_are_rejected_without_plaintext() {
        let ring = ContentKeyring::v1_only(Arc::new(ContentKek::from_bytes([13; 32])));
        let wrong_ring = ContentKeyring::v1_only(Arc::new(ContentKek::from_bytes([14; 32])));
        let id = ObjectId::parse("c0a00000-0001-4000-8000-000000000002").unwrap();
        let sealed = seal_chapter_with_keyring(version(), id, "confidential", &ring).unwrap();
        assert!(
            open_registered_paid_chapter(metadata(&sealed), &sealed.ciphertext, &wrong_ring)
                .is_err(),
            "a different configured KEK cannot read historical ciphertext"
        );
        let mut other = metadata(&sealed);
        other.version = ChapterVersionRef {
            chapter_id: version().chapter_id,
            version_id: PositiveId::new(433).unwrap(),
        };
        assert!(
            open_registered_paid_chapter(other, &sealed.ciphertext, &ring).is_err(),
            "an adjacent chapter edition cannot reuse the original ciphertext"
        );
    }

    #[test]
    fn retained_v1_works_after_v2_key_activation() {
        let v1 = Arc::new(ContentKek::from_bytes([21; 32]));
        let v2 = Arc::new(ContentKek::from_bytes([22; 32]));
        let ring = ContentKeyring::v1_only(v1.clone());
        let id = ObjectId::parse("c0a00000-0001-4000-8000-000000000003").unwrap();
        let old = seal_chapter_with_keyring(version(), id, "Историческая V1", &ring).unwrap();
        let rotated = ContentKeyring::with_v2(v1, v2)
            .unwrap()
            .with_active_id("content-kek-v2")
            .unwrap();
        let opened =
            open_registered_paid_chapter(metadata(&old), &old.ciphertext, &rotated).unwrap();
        assert_eq!(opened.as_str(), "Историческая V1");
    }
}
