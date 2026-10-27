//! G S065: a paid image gets an independent key, nonce and immutable
//! version-bound AAD. This is NOT a public renderer or a paid entitlement.
use super::*;

/// Strictly recognized source media. The browser must also validate the
/// decrypted MIME before rendering and never insert arbitrary active content.
fn valid_media(bytes: &[u8], mime: &str) -> bool {
    if bytes.len() < 16 || bytes.len() > 8 * 1024 * 1024 {
        return false;
    }
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff])
            && bytes.ends_with(&[0xff, 0xd9]),
        "image/webp" => bytes.starts_with(b"RIFF")
            && bytes.get(8..12) == Some(&b"WEBP"[..]),
        _ => false,
    }
}

pub fn asset_aad(
    version: ChapterVersionRef,
    object_id: &ObjectId,
    plaintext_sha256: &[u8; 32],
) -> Vec<u8> {
    format!(
        "tor.encrypted.asset.v1\n{}\n{}\n{}\n{}\n",
        version.chapter_id.get(),
        version.version_id.get(),
        object_id.as_str(),
        hex::encode(plaintext_sha256),
    )
    .into_bytes()
}

/// G-only output awaiting verified S3 PUT/GET and a B-owned atomic attach.
/// No underlying image bytes or raw key persist in the PostgreSQL registry.
pub struct EncryptedAsset {
    pub object_id: ObjectId,
    pub version: ChapterVersionRef,
    pub mime: &'static str,
    pub ciphertext: Vec<u8>,
    pub object_nonce: [u8; 12],
    pub aad: Vec<u8>,
    pub ciphertext_sha256: [u8; 32],
    pub plaintext_sha256: [u8; 32],
    pub wrap_nonce: [u8; 12],
    pub wrapped_dek: Vec<u8>,
    pub kek_id: &'static str,
}

impl fmt::Debug for EncryptedAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncryptedAsset")
            .field("object_id", &self.object_id)
            .field("version", &self.version)
            .field("mime", &self.mime)
            .field("ciphertext_len", &self.ciphertext.len())
            .field("kek_id", &self.kek_id)
            .finish_non_exhaustive()
    }
}

pub fn seal_asset(
    version: ChapterVersionRef,
    object_id: ObjectId,
    image: &[u8],
    mime: &'static str,
    kek: &ContentKek,
) -> Result<EncryptedAsset, CryptoError> {
    seal_with_key(version, object_id, image, mime, kek, KEK_ID)
}

#[cfg(any(feature = "pg", test))]
pub fn seal_asset_with_keyring(
    version: ChapterVersionRef,
    object_id: ObjectId,
    image: &[u8],
    mime: &'static str,
    ring: &ContentKeyring,
) -> Result<EncryptedAsset, CryptoError> {
    seal_with_key(
        version,
        object_id,
        image,
        mime,
        ring.active_key()?,
        ring.active_id(),
    )
}

fn seal_with_key(
    version: ChapterVersionRef,
    object_id: ObjectId,
    image: &[u8],
    mime: &'static str,
    kek: &ContentKek,
    kek_id: &'static str,
) -> Result<EncryptedAsset, CryptoError> {
    if !valid_media(image, mime) {
        return Err(CryptoError::InvalidMaterial);
    }
    let plaintext_sha256 = sha256_bytes(image);
    let aad = asset_aad(version, &object_id, &plaintext_sha256);
    let dek = Zeroizing::new(random_bytes::<32>()?);
    let object_nonce = random_bytes::<12>()?;
    let wrap_nonce = random_bytes::<12>()?;
    let ciphertext = encrypt_raw(&dek, &object_nonce, &aad, image)?;
    let wrapped_dek = encrypt_raw(&kek.0, &wrap_nonce, &wrap_aad(&object_id), &dek[..])?;
    let checked = unwrap_bytes(&object_id, &wrapped_dek, &wrap_nonce, kek)?;
    if wrapped_dek.len() != 48 || checked.as_slice() != &dek[..] {
        return Err(CryptoError::AuthenticationFailed);
    }
    Ok(EncryptedAsset {
        object_id,
        version,
        mime,
        ciphertext_sha256: sha256_bytes(&ciphertext),
        ciphertext,
        object_nonce,
        aad,
        plaintext_sha256,
        wrap_nonce,
        wrapped_dek,
        kek_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_contracts::paid_delivery::PositiveId;

    fn pin(version_id: i64) -> ChapterVersionRef {
        ChapterVersionRef {
            chapter_id: PositiveId::new(51).unwrap(),
            version_id: PositiveId::new(version_id).unwrap(),
        }
    }

    #[test]
    fn image_aad_is_immutable_to_exact_version_and_not_chapter_aad() {
        let id = ObjectId::parse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        let kek = ContentKek::from_bytes([7; 32]);
        let mut image = b"\x89PNG\r\n\x1a\n".to_vec();
        image.extend_from_slice(&[0; 40]);
        let first = seal_asset(pin(11), id.clone(), &image, "image/png", &kek).unwrap();
        let second = seal_asset(pin(11), id.clone(), &image, "image/png", &kek).unwrap();
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(first.object_nonce, second.object_nonce);
        assert_ne!(first.wrap_nonce, second.wrap_nonce);
        assert_ne!(first.aad, asset_aad(pin(12), &id, &first.plaintext_sha256));
        assert_eq!(first.aad, asset_aad(pin(11), &id, &first.plaintext_sha256));
        assert_eq!(
            decrypt_raw(
                &unwrap_bytes(&id, &first.wrapped_dek, &first.wrap_nonce, &kek).unwrap(),
                &first.object_nonce, &first.aad, &first.ciphertext
            ).unwrap(),
            image
        );
        assert!(!format!("{first:?}").contains(&hex::encode(&image)));
        let secret = unwrap_bytes(&id, &first.wrapped_dek, &first.wrap_nonce, &kek).unwrap();
        let swapped_version = asset_aad(pin(12), &id, &first.plaintext_sha256);
        assert_eq!(
            decrypt_raw(&secret, &first.object_nonce, &swapped_version, &first.ciphertext),
            Err(CryptoError::AuthenticationFailed),
            "V1 ciphertext is not valid with the V2 AAD",
        );
        let mut corrupt = first.ciphertext.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert_eq!(
            decrypt_raw(&secret, &first.object_nonce, &first.aad, &corrupt),
            Err(CryptoError::AuthenticationFailed),
        );
    }

    #[test]
    fn invalid_image_formats_and_oversized_images_fail_before_encrypt() {
        let id = ObjectId::parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        let kek = ContentKek::from_bytes([9; 32]);
        for (mime, bytes) in [
            ("image/svg+xml", b"<svg>unsafe</svg>".to_vec()),
            ("image/png", b"<script>unsafe</script>".to_vec()),
            ("image/jpeg", b"RIFFxxxxWEBPxxxx".to_vec()),
            ("image/webp", b"\x89PNG\r\n\x1a\nnope".to_vec()),
            ("image/png", vec![0; 8 * 1024 * 1024 + 1]),
        ] {
            assert!(matches!(
                seal_asset(pin(11), id.clone(), &bytes, mime, &kek),
                Err(CryptoError::InvalidMaterial)
            ));
        }
    }
}
