//! Operator-only encrypted connection metadata. Not a live storage switch.
//! The master key is separate from S3 keys and MUST survive PG backup/restore.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sqlx::PgPool;
use zeroize::{Zeroize, Zeroizing};

const AAD: &[u8] = b"tor:s3-admin:yandex:v1";
pub const YANDEX_ENDPOINT: &str = "https://storage.yandexcloud.net";
pub const YANDEX_REGION: &str = "ru-central1";

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
        let (nonce, ciphertext) = encrypt(&key, access, secret)?;
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
                (id,bucket,ciphertext,nonce,updated_by_user_id,verified_at)
             VALUES(1,$1,$2,$3,$4,now())
             ON CONFLICT(id) DO UPDATE SET
                bucket=EXCLUDED.bucket,
                ciphertext=EXCLUDED.ciphertext,
                nonce=EXCLUDED.nonce,
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
        || !bytes[bytes.len() - 1].is_ascii_lowercase()
            && !bytes[bytes.len() - 1].is_ascii_digit()
        || bucket.contains("..") || bucket.contains(".-") || bucket.contains("-.")
    {
        return false;
    }
    bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-. ".contains(b) && *b != b' ')
        && !bucket.split('.').all(|part| part.parse::<u8>().is_ok())
}

pub fn valid_access_key(value: &str) -> bool {
    (8..=128).contains(&value.len()) && value.is_ascii() && !value.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
}

pub fn valid_secret_key(value: &str) -> bool {
    (16..=256).contains(&value.len()) && value.is_ascii() && !value.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
}

fn master_key_from_env() -> Result<Zeroizing<[u8; 32]>, AdminS3Error> {
    let input = Zeroizing::new(std::env::var("TOR_S3_ADMIN_MASTER_KEY")
        .map_err(|_| AdminS3Error::MasterKeyMissing)?);
    let decoded = Zeroizing::new(STANDARD.decode(input.as_bytes())
        .map_err(|_| AdminS3Error::MasterKeyMissing)?);
    if decoded.len() != 32 {
        return Err(AdminS3Error::MasterKeyMissing);
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&decoded);
    Ok(key)
}

fn encrypt(key: &[u8; 32], access: &str, secret: &str) -> Result<([u8; 12], Vec<u8>), AdminS3Error> {
    let mut nonce = [0_u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|_| AdminS3Error::Unavailable)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AdminS3Error::Invalid)?;
    let mut plaintext = Zeroizing::new(serde_json::to_vec(&serde_json::json!({
        "access_key_id": access, "secret_access_key": secret
    })).map_err(|_| AdminS3Error::Invalid)?);
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce),
        Payload { msg: &plaintext, aad: AAD })
        .map_err(|_| AdminS3Error::Unavailable)?;
    plaintext.zeroize();
    Ok((nonce, ciphertext))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_yandex_bucket_and_keys_without_network() {
        assert!(valid_bucket("tore"));
        for bad in ["", "ab", "a..b", "UPPER", "bucket/path", "192.168.1.1", "bad bucket"] {
            assert!(!valid_bucket(bad), "{bad}");
        }
        assert!(valid_access_key("ABCD12345678"));
        assert!(valid_secret_key("abcdef0123456789"));
        assert!(!valid_secret_key("bad\nsecret with spaces"));
    }
    #[test]
    fn aes_256_gcm_binds_ciphertext_to_profile_and_detects_tampering() {
        let key = [11_u8; 32];
        let (nonce, ciphertext) = encrypt(&key,"AKID12345678","not-the-real-secret-value").unwrap();
        assert!(!String::from_utf8_lossy(&ciphertext).contains("not-the-real-secret"));
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let plaintext = cipher.decrypt(Nonce::from_slice(&nonce),
            Payload { msg: &ciphertext, aad: AAD }).unwrap();
        assert!(String::from_utf8(plaintext).unwrap().contains("AKID12345678"));
        let mut modified = ciphertext;
        modified[0] ^= 1;
        assert!(cipher.decrypt(Nonce::from_slice(&nonce),
            Payload { msg: &modified, aad: AAD }).is_err());
        assert!(cipher.decrypt(Nonce::from_slice(&nonce),
            Payload { msg: &modified, aad: b"tor:other-account" }).is_err());
    }
}
