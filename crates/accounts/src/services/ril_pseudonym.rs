//! A-owned, domain-separated HMAC identity for opt-in RIL paths.
//! This type is never serializable, loggable, stored in PG, or used for access.
use hmac::{Hmac, Mac};
use sha2::Sha256;

#[derive(Clone)]
pub struct RilSubjectSigner {
    key: [u8; 32],
}

impl RilSubjectSigner {
    /// A separately provisioned 256-bit secret, represented as 64 hex chars.
    /// Invalid/missing configuration disables identified RIL: fail closed.
    pub fn from_hex(value: &str) -> Option<Self> {
        if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut key = [0_u8; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
        }
        // An all-zero secret is a configuration error, not a valid key.
        if key.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self { key })
    }

    /// Only call after checking the account's active PG session AND explicit
    /// behavior consent. Epoch changes on opt-out and opt-in, breaking linkage.
    pub fn pseudonym(&self, verified_user_id: i64, consent_epoch: i64) -> [u8; 16] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts 256-bit key");
        mac.update(b"tor/ril/behavior/subject/v1");
        mac.update(&verified_user_id.to_be_bytes());
        mac.update(&consent_epoch.to_be_bytes());
        let mut output = [0_u8; 16];
        output.copy_from_slice(&mac.finalize().into_bytes()[..16]);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_real_secret_and_rotates_subject_on_every_consent_epoch() {
        assert!(RilSubjectSigner::from_hex("abc").is_none());
        assert!(RilSubjectSigner::from_hex(&"00".repeat(32)).is_none());
        assert!(RilSubjectSigner::from_hex(&"!!".repeat(32)).is_none());
        let key = RilSubjectSigner::from_hex(&"4f".repeat(32)).unwrap();
        let id = key.pseudonym(999, 1);
        assert_ne!(id, [0; 16]);
        assert_ne!(id, key.pseudonym(999, 2));
        assert_ne!(id, key.pseudonym(998, 1));
        let different_key = RilSubjectSigner::from_hex(&"5a".repeat(32)).unwrap();
        assert_ne!(id, different_key.pseudonym(999, 1));
        assert_eq!(id, key.pseudonym(999, 1));
    }
}
