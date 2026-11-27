//! Offline encrypted candidate. No SMTP, environment lookup or activation.
use super::postbox::valid_id;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAX_SECRET: usize = 1024;
const MAX_CIPHERTEXT: usize = 2 * MAX_SECRET + 4 + 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealError {
    Invalid,
    KeyUnavailable,
    Unavailable,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProfileIdentity {
    identity_id: String,
    sender: String,
}

impl ProfileIdentity {
    pub fn new(identity_id: String, sender: String) -> Result<Self, SealError> {
        if !valid_id(&identity_id) || !valid_sender(&sender) {
            return Err(SealError::Invalid);
        }
        Ok(Self {
            identity_id,
            sender,
        })
    }

    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }

    pub fn sender(&self) -> &str {
        &self.sender
    }
}

fn valid_sender(value: &str) -> bool {
    if value.len() > 254 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        && domain.contains('.')
        && domain
            .rsplit('.')
            .next()
            .is_some_and(|suffix| !suffix.bytes().all(|b| b.is_ascii_digit()))
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// No Debug or Serialize. Only the trusted transport may use these values.
pub struct ProfileSecrets {
    access: Zeroizing<String>,
    secret: Zeroizing<String>,
}

impl ProfileSecrets {
    pub fn new(access: String, secret: String) -> Result<Self, SealError> {
        let access = Zeroizing::new(access);
        let secret = Zeroizing::new(secret);
        if !valid_secret(&access) || !valid_secret(&secret) {
            return Err(SealError::Invalid);
        }
        Ok(Self { access, secret })
    }

    pub fn access(&self) -> &str {
        &self.access
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }
}

fn valid_secret(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SECRET
        && value.bytes().all(|b| b.is_ascii_graphic())
}

/// A versioned key supplied by independently managed operator configuration.
/// Never derive this from the S3, quota, HMAC or database credentials.
pub struct ProfileMasterKey {
    version: i32,
    bytes: Zeroizing<[u8; 32]>,
}

impl ProfileMasterKey {
    pub fn from_base64(version: i32, encoded: &str) -> Result<Self, SealError> {
        let decoded = Zeroizing::new(
            STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| SealError::KeyUnavailable)?,
        );
        let mut bytes = Zeroizing::new([0; 32]);
        if version <= 0 || decoded.len() != bytes.len() || decoded.iter().all(|b| *b == 0) {
            return Err(SealError::KeyUnavailable);
        }
        bytes.copy_from_slice(&decoded);
        Ok(Self { version, bytes })
    }

    pub fn seal(
        &self,
        identity: ProfileIdentity,
        revision: i64,
        secrets: &ProfileSecrets,
    ) -> Result<SealedProfile, SealError> {
        if revision <= 1 {
            return Err(SealError::Invalid);
        }
        let mut nonce = [0; 12];
        OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| SealError::Unavailable)?;
        let mut plaintext = Zeroizing::new(Vec::new());
        for value in [secrets.access(), secrets.secret()] {
            let len = u16::try_from(value.len()).map_err(|_| SealError::Invalid)?;
            plaintext.extend_from_slice(&len.to_be_bytes());
            plaintext.extend_from_slice(value.as_bytes());
        }
        let mut profile = SealedProfile {
            identity,
            revision,
            key_version: self.version,
            nonce,
            ciphertext: Vec::new(),
        };
        let cipher = Aes256Gcm::new_from_slice(self.bytes.as_slice())
            .map_err(|_| SealError::KeyUnavailable)?;
        profile.ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&profile.nonce),
                Payload {
                    msg: &plaintext,
                    aad: &profile.aad(),
                },
            )
            .map_err(|_| SealError::Unavailable)?;
        Ok(profile)
    }

    pub fn open(&self, profile: &SealedProfile) -> Result<ProfileSecrets, SealError> {
        if self.version != profile.key_version || !profile.valid() {
            return Err(SealError::KeyUnavailable);
        }
        let cipher = Aes256Gcm::new_from_slice(self.bytes.as_slice())
            .map_err(|_| SealError::KeyUnavailable)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&profile.nonce),
                    Payload {
                        msg: &profile.ciphertext,
                        aad: &profile.aad(),
                    },
                )
                .map_err(|_| SealError::KeyUnavailable)?,
        );
        let (access, rest) = decode_field(&plaintext)?;
        let (secret, rest) = decode_field(rest)?;
        if !rest.is_empty() || !valid_secret(access) || !valid_secret(secret) {
            return Err(SealError::Invalid);
        }
        ProfileSecrets::new(access.to_owned(), secret.to_owned())
    }
}

fn decode_field(input: &[u8]) -> Result<(&str, &[u8]), SealError> {
    let len = u16::from_be_bytes(
        input
            .get(..2)
            .ok_or(SealError::Invalid)?
            .try_into()
            .map_err(|_| SealError::Invalid)?,
    ) as usize;
    let value = input.get(2..2 + len).ok_or(SealError::Invalid)?;
    let value = std::str::from_utf8(value).map_err(|_| SealError::Invalid)?;
    Ok((value, &input[2 + len..]))
}

pub struct SealedProfile {
    pub(crate) identity: ProfileIdentity,
    pub(crate) revision: i64,
    pub(crate) key_version: i32,
    pub(crate) nonce: [u8; 12],
    pub(crate) ciphertext: Vec<u8>,
}

impl SealedProfile {
    pub fn identity(&self) -> &ProfileIdentity {
        &self.identity
    }

    pub fn revision(&self) -> i64 {
        self.revision
    }

    pub fn key_version(&self) -> i32 {
        self.key_version
    }

    pub(crate) fn valid(&self) -> bool {
        self.revision > 1
            && self.key_version > 0
            && (22..=MAX_CIPHERTEXT).contains(&self.ciphertext.len())
    }

    fn aad(&self) -> Vec<u8> {
        let mut out = b"tor:mail:postbox:profile:v1".to_vec();
        out.extend_from_slice(&self.revision.to_be_bytes());
        out.extend_from_slice(&self.key_version.to_be_bytes());
        for value in [self.identity.identity_id(), self.identity.sender()] {
            out.extend_from_slice(&(value.len() as u64).to_be_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        out
    }

    /// The one-use password proof binds the exact ciphertext, nonce and
    /// metadata, not a plaintext credential hash. Re-encrypting changes it.
    pub fn intent_digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"tor:mail:postbox:save-candidate:v1");
        hash.update(self.aad());
        hash.update(self.nonce);
        hash.update(&self.ciphertext);
        hash.finalize().into()
    }
}

#[cfg(test)]
mod tests;
