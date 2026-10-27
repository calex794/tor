//! Durable S3 orphan cleanup identity. Only G repository may produce a claim.
use super::{
    object_keys::PrivateObjectKey,
    provider::{valid_private_bucket, StorageError},
};
use tor_contracts::paid_delivery::{EncryptedObjectKind, ObjectId};

#[derive(Clone, Debug)]
pub struct GcClaim {
    object_id: ObjectId,
    key: PrivateObjectKey,
    bucket: String,
    epoch: i64,
    attempts: i32,
}

impl GcClaim {
    pub(crate) fn from_database(
        raw_id: &str,
        kind: &str,
        raw_key: &str,
        bucket: String,
        epoch: i64,
        attempts: i32,
    ) -> Result<Self, StorageError> {
        let object_id = ObjectId::parse(raw_id).map_err(|_| StorageError::Invalid)?;
        let object_kind = match kind {
            "chapter" => EncryptedObjectKind::Chapter,
            "chapter_asset" => EncryptedObjectKind::ChapterAsset,
            _ => return Err(StorageError::Invalid),
        };
        let key = PrivateObjectKey::for_object(object_kind, &object_id);
        if key.as_str() != raw_key
            || !valid_private_bucket(&bucket)
            || epoch <= 0
            || !(1..=8).contains(&attempts)
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            object_id,
            key,
            bucket,
            epoch,
            attempts,
        })
    }

    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    pub fn key(&self) -> &PrivateObjectKey {
        &self.key
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn epoch(&self) -> i64 {
        self.epoch
    }

    pub fn attempts(&self) -> i32 {
        self.attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_database_claim_with_cross_namespace_or_invalid_scope() {
        let id = "e5a64c40-41ad-4ff0-93f5-5da145bcd710";
        let key = format!("private/chapter-versions/{id}.enc");
        assert!(GcClaim::from_database(id, "chapter", &key, "tor-private-ci".into(), 1, 1).is_ok());
        assert!(
            GcClaim::from_database(id, "chapter_asset", &key, "tor-private-ci".into(), 1, 1)
                .is_err()
        );
        assert!(GcClaim::from_database(id, "chapter", &key, "public-bucket".into(), 1, 1).is_err());
        assert!(
            GcClaim::from_database(id, "chapter", &key, "tor-private-ci".into(), 0, 1).is_err()
        );
        assert!(
            GcClaim::from_database(id, "chapter", &key, "tor-private-ci".into(), 1, 9).is_err()
        );
    }
}
