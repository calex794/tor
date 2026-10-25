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
