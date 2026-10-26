use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidIdempotencyKey;

impl fmt::Display for InvalidIdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected 43 unpadded base64url characters")
    }
}

impl Error for InvalidIdempotencyKey {}

/// Validated command key shared by HTML forms and JSON command adapters.
///
/// Generate from 32 random bytes. Validation preserves the existing command
/// table grammar; it cannot establish a caller's source of randomness. Replay
/// scope and payload comparison must still be enforced by the owning service.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for IdempotencyKey {
    type Err = InvalidIdempotencyKey;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        validate(raw)?;
        Ok(Self(raw.to_owned()))
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = InvalidIdempotencyKey;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        validate(&raw)?;
        Ok(Self(raw))
    }
}

fn validate(raw: &str) -> Result<(), InvalidIdempotencyKey> {
    if raw.len() == 43
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(())
    } else {
        Err(InvalidIdempotencyKey)
    }
}

impl From<IdempotencyKey> for String {
    fn from(key: IdempotencyKey) -> Self {
        key.0
    }
}
