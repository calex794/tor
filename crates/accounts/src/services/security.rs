//! Password and session-token primitives. Never log either secret.
//! Password hashing is CPU-intensive: web handlers must call it via spawn_blocking.
#![forbid(unsafe_code)]

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const SESSION_TOKEN_BYTES: usize = 32;

/// Uses the Argon2id default parameters and an OS-generated unique salt.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

/// A malformed hash and a wrong password are both rejected.
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}


/// An in-memory proof that a password was freshly verified against one exact
/// stored credential hash. It carries no password and is intentionally not
/// serializable or debuggable.
pub struct VerifiedPassword {
    stored_hash_digest: [u8; 32],
    verified_at: Instant,
}

impl VerifiedPassword {
    /// Step-up verification must be followed by proof issuance promptly.
    pub fn is_fresh(&self) -> bool {
        self.verified_at.elapsed() <= Duration::from_secs(30)
    }

    /// Reject a credential rotation between Argon2 verification and issuance.
    pub fn matches_hash(&self, stored_hash: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(stored_hash.as_bytes()).into();
        digest == self.stored_hash_digest
    }
}

/// Verify a password for a high-risk action. Call from the bounded password
/// worker; the returned capability expires after 30 seconds.
pub fn verify_password_for_step_up(
    password: &str,
    stored_hash: &str,
) -> Option<VerifiedPassword> {
    if password.is_empty() || password.len() > 512 || !verify_password(password, stored_hash) {
        return None;
    }
    Some(VerifiedPassword {
        stored_hash_digest: Sha256::digest(stored_hash.as_bytes()).into(),
        verified_at: Instant::now(),
    })
}

/// 256 unpredictable bits, encoded for a cookie; never persist the token itself.
pub fn generate_session_token() -> String {
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Stable SHA-256 lookup key is safe because session tokens have 256 bits of entropy.
pub fn session_token_digest(token: &str) -> [u8; 32] {
    let output = Sha256::digest(token.as_bytes());
    output.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_salted_and_never_plaintext() {
        let first = hash_password("Достаточно длинный пароль 1").unwrap();
        let second = hash_password("Достаточно длинный пароль 1").unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("$argon2id$"));
        assert!(!first.contains("Достаточно длинный пароль 1"));
        assert!(verify_password("Достаточно длинный пароль 1", &first));
        assert!(!verify_password("неверный пароль", &first));
        assert!(!verify_password("любой", "malformed"));
    }

    #[test]
    fn step_up_password_proof_is_fresh_and_bound_to_the_verified_hash() {
        let hash = hash_password("ModeratorStepUpPassword2026").unwrap();
        let proof = verify_password_for_step_up("ModeratorStepUpPassword2026", &hash).unwrap();
        assert!(proof.is_fresh());
        assert!(proof.matches_hash(&hash));
        let rotated = hash_password("DifferentModeratorPassword2026").unwrap();
        assert!(!proof.matches_hash(&rotated));
        assert!(verify_password_for_step_up("wrong password", &hash).is_none());
        assert!(verify_password_for_step_up("", &hash).is_none());
    }

    #[test]
    fn session_secrets_are_unguessable_and_hashes_differ() {
        let first = generate_session_token();
        let second = generate_session_token();
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
        assert_ne!(session_token_digest(&first), session_token_digest(&second));
        assert_ne!(session_token_digest(&first), [0; 32]);
        assert!(!std::str::from_utf8(&session_token_digest(&first))
            .unwrap_or_default()
            .contains(&first));
    }
}
