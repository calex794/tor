//! Operator-only encrypted connection metadata. Not a live storage switch.
//! The master key is separate from S3 keys and MUST survive PG backup/restore.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sqlx::PgPool;
use zeroize::Zeroizing;

const AAD_V2: &[u8] = b"tor:s3-admin:yandex:v2:";
const ENVELOPE_VERSION: i16 = 2;
pub const YANDEX_ENDPOINT: &str = "https://storage.yandexcloud.net";
pub const YANDEX_REGION: &str = "ru-central1";

/// In-memory candidate only. No Debug, Clone or serialization: the HTTP
/// presentation layer must continue to use AdminS3View, never this type.
/// The nonce identifies this exact saved candidate for future CAS activation.
pub struct PreparedS3Candidate {
    bucket: String,
    revision: [u8; 12],
    access: Zeroizing<String>,
    secret: Zeroizing<String>,
}

impl PreparedS3Candidate {
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn revision(&self) -> [u8; 12] {
        self.revision
    }

    pub fn access_key_id(&self) -> &str {
        self.access.as_str()
    }

    pub fn secret_access_key(&self) -> &str {
        self.secret.as_str()
    }
}

impl PgAdminS3Profiles {
    pub fn master_key_ready() -> bool {
        master_key_from_env().is_ok()
    }
}

#[derive(Clone)]
pub struct PgAdminS3Profiles {
    pool: PgPool,
}

pub struct AdminS3View {
    pub bucket: String,
    pub verified_utc: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminS3Error {
    Invalid,
    MasterKeyMissing,
    Forbidden,
    Unavailable,
}

impl PgAdminS3Profiles {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only metadata crosses the admin GET boundary, never access IDs,
    /// ciphertext, nonces or reusable keys.
    pub async fn overview(&self) -> Result<Option<AdminS3View>, AdminS3Error> {
        let row: Option<(String, String, bool)> = sqlx::query_as(
            "SELECT bucket,
                to_char(verified_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI') || ' UTC',
                active FROM admin_s3_profiles WHERE id=1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AdminS3Error::Unavailable)?;
        Ok(row.map(|(bucket, verified_utc, active)| AdminS3View {
            bucket,
            verified_utc,
            active,
        }))
    }

    /// Internal candidate preparation, not an activation API. Callers must
    /// supply the independently configured destination bucket. Legacy V1
    /// envelopes (not bucket-bound), missing rows, corrupt rows or wrong keys
    /// fail closed: there is no default credential-chain fallback here.
    pub async fn load_verified_candidate(
        &self,
        expected_bucket: &str,
    ) -> Result<PreparedS3Candidate, AdminS3Error> {
        if !valid_bucket(expected_bucket) {
            return Err(AdminS3Error::Invalid);
        }
        let key = master_key_from_env()?;
        let row: Option<(String, Vec<u8>, Vec<u8>, i16)> = sqlx::query_as(
            "SELECT bucket,nonce,ciphertext,encryption_version
             FROM admin_s3_profiles WHERE id=1 AND bucket=$1 AND NOT active",
        )
        .bind(expected_bucket)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AdminS3Error::Unavailable)?;
        let (bucket, nonce, ciphertext, version) = row.ok_or(AdminS3Error::Unavailable)?;
        decode_candidate(&key, bucket, &nonce, &ciphertext, version)
    }

    /// This method is only called AFTER a successful read-only S3 HeadBucket
    /// with the supplied credentials. It rechecks the live administrator
    /// session *inside* the write transaction, with locks against role revoke.
    pub async fn save_verified(
        &self,
        digest: &[u8; 32],
        bucket: &str,
        access: &str,
        secret: &str,
    ) -> Result<(), AdminS3Error> {
        if !valid_bucket(bucket) || !valid_access_key(access) || !valid_secret_key(secret) {
            return Err(AdminS3Error::Invalid);
        }
        let key = master_key_from_env()?;
        let (nonce, ciphertext) = encrypt(&key, bucket, access, secret)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AdminS3Error::Unavailable)?;
        let actor: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM users u
             JOIN sessions s ON s.user_id=u.id
             JOIN account_roles r ON r.user_id=u.id
                AND r.role='administrator' AND r.is_active
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
                AND s.expires_at>now() AND u.account_state='active'
             FOR SHARE OF s,u,r",
        )
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AdminS3Error::Unavailable)?;
        let Some(actor) = actor else {
            return Err(AdminS3Error::Forbidden);
        };
        sqlx::query(
            "INSERT INTO admin_s3_profiles
                (id,bucket,ciphertext,nonce,updated_by_user_id,verified_at,encryption_version)
             VALUES(1,$1,$2,$3,$4,now(),2)
             ON CONFLICT(id) DO UPDATE SET
                bucket=EXCLUDED.bucket,
                ciphertext=EXCLUDED.ciphertext,
                nonce=EXCLUDED.nonce,
                encryption_version=EXCLUDED.encryption_version,
                updated_by_user_id=EXCLUDED.updated_by_user_id,
                verified_at=now(),active=FALSE",
        )
        .bind(bucket)
        .bind(ciphertext)
        .bind(nonce.as_slice())
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| AdminS3Error::Unavailable)?;
        sqlx::query(
            "INSERT INTO admin_s3_profile_audit(performed_by_user_id,action)
             VALUES($1,'save_verified_connection')",
        )
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| AdminS3Error::Unavailable)?;
        tx.commit().await.map_err(|_| AdminS3Error::Unavailable)
    }
}

pub fn valid_bucket(bucket: &str) -> bool {
    let bytes = bucket.as_bytes();
    if !(3..=63).contains(&bytes.len())
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit()
        || bucket.contains("..")
        || bucket.contains(".-")
        || bucket.contains("-.")
    {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.')
        && !bucket.split('.').all(|part| part.parse::<u8>().is_ok())
}

pub fn valid_access_key(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value.is_ascii()
        && !value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
}

pub fn valid_secret_key(value: &str) -> bool {
    (16..=256).contains(&value.len())
        && value.is_ascii()
        && !value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
}

fn master_key_from_env() -> Result<Zeroizing<[u8; 32]>, AdminS3Error> {
    let input = Zeroizing::new(
        std::env::var("TOR_S3_ADMIN_MASTER_KEY").map_err(|_| AdminS3Error::MasterKeyMissing)?,
    );
    let decoded = Zeroizing::new(
        STANDARD
            .decode(input.as_bytes())
            .map_err(|_| AdminS3Error::MasterKeyMissing)?,
    );
    if decoded.len() != 32 {
        return Err(AdminS3Error::MasterKeyMissing);
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&decoded);
    Ok(key)
}

fn aad(bucket: &str) -> Vec<u8> {
    [AAD_V2, bucket.as_bytes()].concat()
}

fn encrypt(
    key: &[u8; 32],
    bucket: &str,
    access: &str,
    secret: &str,
) -> Result<([u8; 12], Vec<u8>), AdminS3Error> {
    if !valid_bucket(bucket) || !valid_access_key(access) || !valid_secret_key(secret) {
        return Err(AdminS3Error::Invalid);
    }
    let mut nonce = [0_u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|_| AdminS3Error::Unavailable)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AdminS3Error::Invalid)?;
    // Bounded binary envelope avoids serde allocations of plaintext keys.
    // The temporary buffer is wiped even if encryption fails.
    let mut plaintext = Zeroizing::new(Vec::with_capacity(2 + access.len() + secret.len()));
    plaintext.extend_from_slice(&(access.len() as u16).to_be_bytes());
    plaintext.extend_from_slice(access.as_bytes());
    plaintext.extend_from_slice(secret.as_bytes());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &aad(bucket),
            },
        )
        .map_err(|_| AdminS3Error::Unavailable)?;
    Ok((nonce, ciphertext))
}

fn decode_candidate(
    key: &[u8; 32],
    bucket: String,
    nonce: &[u8],
    ciphertext: &[u8],
    version: i16,
) -> Result<PreparedS3Candidate, AdminS3Error> {
    if version != ENVELOPE_VERSION
        || !valid_bucket(&bucket)
        || !(42..=402).contains(&ciphertext.len())
    {
        return Err(AdminS3Error::Unavailable);
    }
    let revision: [u8; 12] = nonce.try_into().map_err(|_| AdminS3Error::Unavailable)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AdminS3Error::Unavailable)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&revision),
                Payload {
                    msg: ciphertext,
                    aad: &aad(&bucket),
                },
            )
            .map_err(|_| AdminS3Error::Unavailable)?,
    );
    let length_bytes = plaintext.get(..2).ok_or(AdminS3Error::Unavailable)?;
    let access_len = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    let split = 2 + access_len;
    let access = std::str::from_utf8(plaintext.get(2..split).ok_or(AdminS3Error::Unavailable)?)
        .map_err(|_| AdminS3Error::Unavailable)?;
    let secret = std::str::from_utf8(plaintext.get(split..).ok_or(AdminS3Error::Unavailable)?)
        .map_err(|_| AdminS3Error::Unavailable)?;
    if !valid_access_key(access) || !valid_secret_key(secret) {
        return Err(AdminS3Error::Unavailable);
    }
    Ok(PreparedS3Candidate {
        bucket,
        revision,
        access: Zeroizing::new(access.to_owned()),
        secret: Zeroizing::new(secret.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_yandex_bucket_and_keys_without_network() {
        assert!(valid_bucket("tore"));
        for bad in [
            "",
            "ab",
            "a..b",
            "UPPER",
            "bucket/path",
            "192.168.1.1",
            "bad bucket",
        ] {
            assert!(!valid_bucket(bad), "{bad}");
        }
        assert!(valid_access_key("ABCD12345678"));
        assert!(valid_secret_key("abcdef0123456789"));
        assert!(!valid_secret_key("bad\nsecret with spaces"));
    }
    #[test]
    fn aes_256_gcm_binds_ciphertext_to_profile_and_detects_tampering() {
        let key = [11_u8; 32];
        let bucket = "ci-disposable";
        let access = "AKID12345678";
        let secret = "not-the-real-secret-value";
        let (nonce, ciphertext) = encrypt(&key, bucket, access, secret).unwrap();
        assert!(!String::from_utf8_lossy(&ciphertext).contains(secret));
        let decode = |key, bucket: &str, nonce: &[u8], ciphertext: &[u8], version| {
            decode_candidate(key, bucket.to_owned(), nonce, ciphertext, version)
        };
        let loaded = decode(&key, bucket, &nonce, &ciphertext, ENVELOPE_VERSION).unwrap();
        assert_eq!(loaded.bucket(), bucket);
        assert_eq!(loaded.revision(), nonce);
        assert!(loaded.access_key_id() == access);
        assert!(loaded.secret_access_key() == secret);
        assert!(decode(&[12_u8; 32], bucket, &nonce, &ciphertext, 2).is_err());
        assert!(decode(&key, "other-bucket", &nonce, &ciphertext, 2).is_err());
        assert!(decode(&key, bucket, &nonce[..11], &ciphertext, 2).is_err());
        assert!(decode(&key, bucket, &nonce, &ciphertext, 1).is_err());
        let mut modified = ciphertext;
        modified[0] ^= 1;
        assert!(decode(&key, bucket, &nonce, &modified, 2).is_err());
        let (rotated, _) = encrypt(&key, bucket, access, secret).unwrap();
        assert_ne!(nonce, rotated);
    }
}
