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
#[cfg(any(feature = "pg", test))]
use std::sync::Arc;
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId};
use zeroize::Zeroizing;

const KEK_ID: &str = "content-kek-v1";
const KEK_ID_V2: &str = "content-kek-v2";
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
        Self::from_named_environment("TOR_CONTENT_KEK_V1")
    }

    fn from_named_environment(name: &str) -> Result<Self, CryptoError> {
        let encoded = Zeroizing::new(std::env::var(name).map_err(|_| CryptoError::UnavailableKey)?);
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

/// Read-only, versioned content keys, independent of cookies/JWT and the DB.
/// V1 remains mandatory until no historical edition references it.
#[cfg(any(feature = "pg", test))]
pub struct ContentKeyring {
    v1: Arc<ContentKek>,
    v2: Option<Arc<ContentKek>>,
}

#[cfg(any(feature = "pg", test))]
impl ContentKeyring {
    /// Preserve all existing v1 callers without activating key rotation.
    pub fn v1_only(v1: Arc<ContentKek>) -> Self {
        Self { v1, v2: None }
    }

    pub fn with_v2(v1: Arc<ContentKek>, v2: Arc<ContentKek>) -> Result<Self, CryptoError> {
        if v1.0[..] == v2.0[..] {
            return Err(CryptoError::InvalidMaterial);
        }
        Ok(Self { v1, v2: Some(v2) })
    }

    /// Missing V2 means the old safe V1-only reader. A present but invalid V2
    /// must fail startup rather than silently serving an inconsistent keyring.
    pub fn from_environment() -> Result<Self, CryptoError> {
        Self::from_v1_and_optional_v2_environment(Arc::new(ContentKek::from_environment()?))
    }

    /// G can retain its already loaded V1 without copying key bytes.
    /// Optional V2 comes from the same operator-owned secret environment.
    pub fn from_v1_and_optional_v2_environment(
        v1: Arc<ContentKek>,
    ) -> Result<Self, CryptoError> {
        match std::env::var("TOR_CONTENT_KEK_V2") {
            Ok(_) => Self::with_v2(
                v1,
                Arc::new(ContentKek::from_named_environment("TOR_CONTENT_KEK_V2")?),
            ),
            Err(std::env::VarError::NotPresent) => Ok(Self::v1_only(v1)),
            Err(_) => Err(CryptoError::UnavailableKey),
        }
    }

    fn for_id(&self, id: &str) -> Result<&ContentKek, CryptoError> {
        match id {
            KEK_ID => Ok(&self.v1),
            KEK_ID_V2 => self.v2.as_deref().ok_or(CryptoError::UnavailableKey),
            _ => Err(CryptoError::UnavailableKey),
        }
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
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
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
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
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
    let wrapped_dek = encrypt_raw(&kek.0, &wrap_nonce, &wrap_aad(&object_id), &dek[..])?;
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

/// Only encrypted DEK wrapping metadata can change during a KEK rotation.
/// The chapter ciphertext, content nonce, digest, AAD, object ID and purchase
/// are deliberately not inputs or outputs of this operation.
pub struct RewrappedDekV2 {
    pub wrapped_dek: Vec<u8>,
    pub wrap_nonce: [u8; 12],
    pub kek_id: &'static str,
}

impl fmt::Debug for RewrappedDekV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RewrappedDekV2")
            .field("wrapped_dek", &"[REDACTED]")
            .field("wrap_nonce", &"[REDACTED]")
            .field("kek_id", &self.kek_id)
            .finish()
    }
}

/// S3-34 phase 1: authenticate the v1 wrapper, then protect the exact same
/// scoped DEK with a fresh nonce and an independently supplied v2 KEK.
/// No S3 bytes are read, decrypted or rewritten. The caller must perform
/// an authorized PostgreSQL compare-and-swap after a verified backup.
/// Until that controlled migration exists this function changes NO data.
pub fn rewrap_registered_v1_to_v2(
    object_id: &ObjectId,
    v1_wrapped_dek: &[u8],
    v1_wrap_nonce: &[u8; 12],
    v1_kek: &ContentKek,
    v2_kek: &ContentKek,
) -> Result<RewrappedDekV2, CryptoError> {
    // Reuse of the same 32-byte secret under a new name is not a rotation.
    if v1_kek.0[..] == v2_kek.0[..] {
        return Err(CryptoError::InvalidMaterial);
    }
    let dek = unwrap_registered_dek(object_id, v1_wrapped_dek, v1_wrap_nonce, KEK_ID, v1_kek)?;
    let wrap_nonce = random_bytes::<12>()?;
    let aad = wrap_aad(object_id);
    let wrapped_dek = encrypt_raw(&v2_kek.0, &wrap_nonce, &aad, &dek[..])?;
    // Verify both the authentication tag and the exact DEK before returning
    // any material that could be committed by a future migration.
    let confirmed = Zeroizing::new(decrypt_raw(&v2_kek.0, &wrap_nonce, &aad, &wrapped_dek)?);
    if confirmed.as_slice() != &dek[..] || wrapped_dek.len() != 48 {
        return Err(CryptoError::AuthenticationFailed);
    }
    Ok(RewrappedDekV2 {
        wrapped_dek,
        wrap_nonce,
        kek_id: KEK_ID_V2,
    })
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
    unwrap_registered_dek(
        &encrypted.object_id,
        &encrypted.wrapped_dek,
        &encrypted.wrap_nonce,
        encrypted.kek_id,
        kek,
    )
}

/// G-only. Unwrap the exact persisted object key after a trusted attached
/// registry lookup. Never expose this method through HTTP or accept a user
/// supplied object reference. The result is always zeroized on drop.
pub(crate) fn unwrap_registered_dek(
    object_id: &ObjectId,
    wrapped_dek: &[u8],
    wrap_nonce: &[u8; 12],
    kek_id: &str,
    kek: &ContentKek,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if kek_id != KEK_ID {
        return Err(CryptoError::UnavailableKey);
    }
    unwrap_bytes(object_id, wrapped_dek, wrap_nonce, kek)
}

fn unwrap_bytes(
    object_id: &ObjectId,
    wrapped_dek: &[u8],
    wrap_nonce: &[u8; 12],
    kek: &ContentKek,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if wrapped_dek.len() != 48 {
        return Err(CryptoError::UnavailableKey);
    }
    let aad = wrap_aad(object_id);
    let bytes = Zeroizing::new(decrypt_raw(&kek.0, wrap_nonce, &aad, wrapped_dek)?);
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidMaterial);
    }
    let mut dek = Zeroizing::new([0_u8; 32]);
    dek[..].copy_from_slice(&bytes);
    Ok(dek)
}

/// Select only an independently configured KEK for the trusted registry
/// version. Never guess a key or silently fall back to V1 for a V2 row.
#[cfg(any(feature = "pg", test))]
pub(crate) fn unwrap_registered_dek_from_keyring(
    object_id: &ObjectId,
    wrapped_dek: &[u8],
    wrap_nonce: &[u8; 12],
    kek_id: &str,
    keyring: &ContentKeyring,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let key = keyring.for_id(kek_id)?;
    unwrap_bytes(object_id, wrapped_dek, wrap_nonce, key)
}

#[cfg(test)]
mod tests;
