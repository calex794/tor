//! Unpredictable, independent per-card proofs. They encode no account or browser identity.
use crate::domain::exposure::{ExposureError, IssuedObservation};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use tor_contracts::exposure::CardObservation;

pub fn digest(proof: &str) -> Result<[u8; 32], ExposureError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(proof)
        .map_err(|_| ExposureError::Invalid)?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != proof {
        return Err(ExposureError::Invalid);
    }
    Ok(Sha256::digest(bytes).into())
}

/// Independent idempotency key for one worker registration attempt.
pub fn worker_registration_key() -> Result<String, ExposureError> {
    let mut bytes = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ExposureError::Unavailable)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub fn issue(card: CardObservation) -> Result<(String, IssuedObservation), ExposureError> {
    if !card.valid() {
        return Err(ExposureError::Invalid);
    }
    let mut bytes = [0; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ExposureError::Unavailable)?;
    let proof = URL_SAFE_NO_PAD.encode(bytes);
    Ok((
        proof,
        IssuedObservation {
            digest: Sha256::digest(bytes).into(),
            card,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_contracts::exposure::ExposureSurface;
    #[test]
    fn proofs_are_independent_canonical_and_actor_free() {
        let card = CardObservation {
            book_id: 1,
            surface: ExposureSurface::Home,
            editorial: None,
        };
        let (a, pa) = issue(card).unwrap();
        let (b, pb) = issue(card).unwrap();
        assert_ne!(a, b);
        assert_ne!(pa.digest, pb.digest);
        assert_eq!(digest(&a).unwrap(), pa.digest);
        assert_eq!(a.len(), 43);
        assert!(digest("invalid").is_err());
    }
}
