//! Opaque 256-bit assignment tokens. Only their digest is stored.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use crate::domain::feedback::FeedbackError;

pub fn new_ticket() -> Result<String, FeedbackError> {
    let mut bytes = [0_u8; 32];
    OsRng.try_fill_bytes(&mut bytes).map_err(|_| FeedbackError::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
pub fn validate_actor(actor: i64) -> Result<(), FeedbackError> {
    if actor <= 0 { return Err(FeedbackError::Invalid); }
    Ok(())
}
pub fn validate_revision(revision: i64) -> Result<(), FeedbackError> {
    if !(0..i64::MAX).contains(&revision) { return Err(FeedbackError::Invalid); }
    Ok(())
}
