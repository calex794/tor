//! S3-06: immutable paid chapter envelope: UTF-8 -> GZIP -> AES-256-GCM.
//! The object DEK is wrapped under an independent external KEK. No raw key
//! or plaintext is persisted in this envelope or returned over public HTTP.
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::{fmt, io::Write};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId};
use zeroize::Zeroizing;

const KEK_ID: &str = "content-kek-v1";
const MAX_CHAPTER_UTF8_BYTES: usize = 200_000;
const MAX_CHAPTER_CHARS: usize = 50_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoError {
    UnavailableKey,
    RandomUnavailable,
    InvalidMaterial,
    AuthenticationFailed,
    CompressionFailed,
}

/// The content KEK is never serialized, logged or stored in PostgreSQL.
pub struct ContentKek(Zeroizing<[u8; 32]>);

impl ContentKek {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Strict unpadded base64url, exactly 32 bytes, from a separate secret.
    /// Never derive this from cookie, JWT, database password or HMAC keys.
    pub fn from_environment() -> Result<Self, CryptoError> {
        let encoded = Zeroizing::new(
            std::env::var("TOR_CONTENT_KEK_V1").map_err(|_| CryptoError::UnavailableKey)?,
        );
        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded.as_bytes())
                .map_err(|_| CryptoError::UnavailableKey)?,
        );
        if decoded.len() != 32 {
            return Err(CryptoError::UnavailableKey);
        }
        let mut secret = Zeroizing::new([0; 32]);
        secret[..].copy_from_slice(&decoded);
        Ok(Self(secret))
    }
}

impl fmt::Debug for ContentKek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContentKek([REDACTED])")
    }
}

/// May be registered with G/PG after HEAD/hash verification. No plaintext DEK.
/// The returned payload is NOT proof that S3 upload or publication succeeded.
pub struct EncryptedChapter {
    pub object_id: ObjectId,
    pub version: ChapterVersionRef,
    pub ciphertext: Vec<u8>,
    pub object_nonce: [u8; 12],
    pub aad: Vec<u8>,
    pub ciphertext_sha256: [u8; 32],
    pub plaintext_sha256: [u8; 32],
    pub wrap_nonce: [u8; 12],
    pub wrapped_dek: Vec<u8>,
    pub kek_id: &'static str,
}

impl fmt::Debug for EncryptedChapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncryptedChapter")
            .field("object_id", &self.object_id)
            .field("version", &self.version)
            .field("ciphertext_len", &self.ciphertext.len())
            .field("kek_id", &self.kek_id)
            .finish_non_exhaustive()
    }
}

pub fn sha256_bytes(input: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(input);
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

pub fn chapter_aad(
    version: ChapterVersionRef,
    object_id: &ObjectId,
    plaintext_sha256: &[u8; 32],
) -> Vec<u8> {
    format!(
        "tor.encrypted.chapter.v1\n{}\n{}\n{}\n{}\n",
        version.chapter_id.get(),
        version.version_id.get(),
        object_id.as_str(),
        hex::encode(plaintext_sha256)
    )
    .into_bytes()
}

pub fn wrap_aad(object_id: &ObjectId) -> Vec<u8> {
    format!("tor.wrap.v1\n{}\n", object_id.as_str()).into_bytes()
}

fn random_bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut bytes = [0_u8; N];
    getrandom::getrandom(&mut bytes).map_err(|_| CryptoError::RandomUnavailable)?;
    Ok(bytes)
}

fn encrypt_raw(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidMaterial)?;
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::AuthenticationFailed)
}

fn decrypt_raw(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidMaterial)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| CryptoError::AuthenticationFailed)
}

fn gzip_markdown(input: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|_| CryptoError::CompressionFailed)?;
    encoder.finish().map_err(|_| CryptoError::CompressionFailed)
}

/// Create fresh independent DEK, object nonce and KEK-wrap nonce using OS RNG.
/// Never accept any of these secrets or nonces from a browser request.
pub fn seal_chapter(
    version: ChapterVersionRef,
    object_id: ObjectId,
    markdown: &str,
    kek: &ContentKek,
) -> Result<EncryptedChapter, CryptoError> {
    let normalized = Zeroizing::new(markdown.replace("\r\n", "\n").replace('\r', "\n"));
    if normalized.is_empty()
        || normalized.len() > MAX_CHAPTER_UTF8_BYTES
        || normalized.chars().count() > MAX_CHAPTER_CHARS
    {
        return Err(CryptoError::InvalidMaterial);
    }
    let plaintext_sha256 = sha256_bytes(normalized.as_bytes());
    let aad = chapter_aad(version, &object_id, &plaintext_sha256);
    let compressed = Zeroizing::new(gzip_markdown(normalized.as_bytes())?);
    let dek = Zeroizing::new(random_bytes::<32>()?);
    let object_nonce = random_bytes::<12>()?;
    let wrap_nonce = random_bytes::<12>()?;
    let ciphertext = encrypt_raw(&dek, &object_nonce, &aad, &compressed)?;
    let wrapped_dek = encrypt_raw(&kek.0, &wrap_nonce, &wrap_aad(&object_id), &dek)?;
    let ciphertext_sha256 = sha256_bytes(&ciphertext);
    let sealed = EncryptedChapter {
        object_id,
        version,
        ciphertext,
        object_nonce,
        aad,
        ciphertext_sha256,
        plaintext_sha256,
        wrap_nonce,
        wrapped_dek,
        kek_id: KEK_ID,
    };
    // Verify newly wrapped DEK before the object can be registered or uploaded.
    if unwrap_scoped_dek(&sealed, kek)?[..] != dek[..] {
        return Err(CryptoError::AuthenticationFailed);
    }
    Ok(sealed)
}

/// Verify downloaded bytes against the immutable PG/HEAD receipt before any
/// decryption. A HEAD alone is never evidence of actual ciphertext integrity.
pub fn verify_ciphertext(encrypted: &EncryptedChapter, bytes: &[u8]) -> bool {
    bytes.len() >= 17
        && bytes.len() == encrypted.ciphertext.len()
        && sha256_bytes(bytes) == encrypted.ciphertext_sha256
}

/// G-internal only. Caller must have a freshly verified D entitlement and
/// exact B version/object membership before this key can be used in a grant.
pub(crate) fn unwrap_scoped_dek(
    encrypted: &EncryptedChapter,
    kek: &ContentKek,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if encrypted.kek_id != KEK_ID {
        return Err(CryptoError::UnavailableKey);
    }
    let aad = wrap_aad(&encrypted.object_id);
    let bytes = Zeroizing::new(decrypt_raw(
        &kek.0,
        &encrypted.wrap_nonce,
        &aad,
        &encrypted.wrapped_dek,
    )?);
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidMaterial);
    }
    let mut dek = Zeroizing::new([0_u8; 32]);
    dek[..].copy_from_slice(&bytes);
    Ok(dek)
}

#[cfg(test)]
mod tests;
