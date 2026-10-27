//! Immutable S3 object keys, derived from trusted canonical object IDs.
//! A browser-provided key, hostname, bucket, or path is never a grant input.
use tor_contracts::paid_delivery::{ContractError, EncryptedObjectKind, ObjectId};

pub const ENCRYPTED_CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateObjectKey(String);

impl PrivateObjectKey {
    pub fn for_object(kind: EncryptedObjectKind, id: &ObjectId) -> Self {
        let prefix = match kind {
            EncryptedObjectKind::Chapter => "private/chapter-versions/",
            EncryptedObjectKind::ChapterAsset => "private/chapter-assets/",
        };
        Self(format!("{prefix}{}.enc", id.as_str()))
    }

    /// Parse only an exact, lower-case canonical object path. No URL-decoding,
    /// query suffix, slashes or escaping can introduce a second namespace.
    pub fn parse(raw: &str) -> Result<Self, ContractError> {
        let (kind, remainder) = if let Some(id) = raw.strip_prefix("private/chapter-versions/") {
            (EncryptedObjectKind::Chapter, id)
        } else if let Some(id) = raw.strip_prefix("private/chapter-assets/") {
            (EncryptedObjectKind::ChapterAsset, id)
        } else {
            return Err(ContractError::InvalidObjectId);
        };
        let bare_id = remainder
            .strip_suffix(".enc")
            .ok_or(ContractError::InvalidObjectId)?;
        let id = ObjectId::parse(bare_id)?;
        let key = Self::for_object(kind, &id);
        if key.as_str() != raw {
            return Err(ContractError::InvalidObjectId);
        }
        Ok(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
