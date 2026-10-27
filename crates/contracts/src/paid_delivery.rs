//! S3-02: trusted, fail-closed interfaces for private paid delivery.
//! Client-supplied version IDs and S3 object keys are never entitlement facts.
use async_trait::async_trait;
use serde::Serialize;
use std::{fmt, time::Duration};

pub const DEFAULT_SIGNED_GET_TTL: Duration = Duration::from_secs(90);
pub const MAX_SIGNED_GET_TTL: Duration = Duration::from_secs(120);
pub const MIN_SIGNED_GET_TTL: Duration = Duration::from_secs(30);
pub const PAID_DELIVERY_SCHEMA: &str = "tor.paid_delivery.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    InvalidId,
    InvalidObjectId,
    InvalidCiphertext,
    InvalidTtl,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PositiveId(i64);

impl PositiveId {
    pub fn new(value: i64) -> Result<Self, ContractError> {
        if value <= 0 {
            return Err(ContractError::InvalidId);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }

    pub fn wire(self) -> String {
        self.0.to_string()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Actor {
    pub user_id: PositiveId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChapterVersionRef {
    pub chapter_id: PositiveId,
    pub version_id: PositiveId,
}

/// A purchase grants exactly the edition returned by the D entitlement port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntitlementDecision {
    Allowed { pinned: ChapterVersionRef },
    Denied { reason: DenialReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenialReason {
    NotPurchased,
    Refunded,
    Locked,
    Hidden,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryError {
    Unauthenticated,
    Denied,
    Hidden,
    Conflict,
    RateLimited,
    Unavailable,
    Invalid,
}

pub fn validate_signed_get_ttl(ttl: Duration) -> Result<Duration, ContractError> {
    if !(MIN_SIGNED_GET_TTL..=MAX_SIGNED_GET_TTL).contains(&ttl) {
        return Err(ContractError::InvalidTtl);
    }
    Ok(ttl)
}

/// Canonical lowercase hyphenated UUID; never a browser-controlled path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn parse(raw: &str) -> Result<Self, ContractError> {
        let bytes = raw.as_bytes();
        if bytes.len() != 36 {
            return Err(ContractError::InvalidObjectId);
        }
        for (index, byte) in bytes.iter().enumerate() {
            if [8, 13, 18, 23].contains(&index) {
                if *byte != b'-' {
                    return Err(ContractError::InvalidObjectId);
                }
            } else if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte) {
                return Err(ContractError::InvalidObjectId);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncryptedObjectKind {
    Chapter,
    ChapterAsset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentCodec {
    Gzip,
    None,
}

impl ContentCodec {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::None => "none",
        }
    }
}

/// Immutable registry reference. Secrets and plaintext are not stored here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectRef {
    pub object_id: ObjectId,
    pub kind: EncryptedObjectKind,
    pub sha256_ciphertext: [u8; 32],
    pub size_bytes: u64,
}

impl ObjectRef {
    pub fn new(
        object_id: ObjectId,
        kind: EncryptedObjectKind,
        sha256_ciphertext: [u8; 32],
        size_bytes: u64,
    ) -> Result<Self, ContractError> {
        // A GCM authentication tag alone is 16 bytes; never sign an empty
        // pending registry record or an uninitialized all-zero digest.
        if size_bytes < 17 || sha256_ciphertext == [0; 32] {
            return Err(ContractError::InvalidCiphertext);
        }
        Ok(Self {
            object_id,
            kind,
            sha256_ciphertext,
            size_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedObjectRef {
    pub object: ObjectRef,
    pub nonce: [u8; 12],
    pub aad: Vec<u8>,
    pub wrapped_key_id: String,
    pub codec: ContentCodec,
}

/// Signed URL and scoped DEK must not appear in Debug/trace logs.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: String) -> Result<Self, ContractError> {
        if value.is_empty() || value.len() > 4096 {
            return Err(ContractError::InvalidCiphertext);
        }
        Ok(Self(value))
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Serialize)]
pub struct GrantedContent {
    pub ciphertext_url: SecretValue,
    pub object_sha256: String,
    pub cipher: &'static str,
    pub codec: &'static str,
    pub nonce_b64u: String,
    pub aad_b64u: String,
    pub dek_b64u: SecretValue,
}

/// Serialized only into a verified actor's no-store HTTP response.
#[derive(Debug, Serialize)]
pub struct DeliveryGrant {
    pub schema: &'static str,
    pub chapter_id: String,
    pub version_id: String,
    pub expires_at: String,
    pub content: GrantedContent,
    pub protected_mode: &'static str,
    pub assets: Vec<GrantedContent>,
}

impl DeliveryGrant {
    pub fn for_pinned_version(
        pinned: ChapterVersionRef,
        expires_at: String,
        content: GrantedContent,
        assets: Vec<GrantedContent>,
    ) -> Self {
        Self {
            schema: PAID_DELIVERY_SCHEMA,
            chapter_id: pinned.chapter_id.wire(),
            version_id: pinned.version_id.wire(),
            expires_at,
            content,
            protected_mode: "pua_v1",
            assets,
        }
    }
}

/// D is the sole source of entitlement. No version_id from the client.
#[async_trait]
pub trait EntitlementPort: Send + Sync {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError>;
}

/// B resolves the immutable object belonging to the trusted pinned version.
#[async_trait]
pub trait ChapterVersionPort: Send + Sync {
    async fn encrypted_chapter(
        &self,
        pinned: ChapterVersionRef,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError>;
}

/// B alone resolves an image UUID from an immutable purchased edition.
/// This is an association check, never a reader entitlement; C MUST call D
/// before and after signing and may not choose a version from the client.
#[async_trait]
pub trait ChapterAssetPort: Send + Sync {
    async fn encrypted_asset(
        &self,
        pinned: ChapterVersionRef,
        object_id: &ObjectId,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError>;
}

/// G checks attached private object state before signing; never accepts an
/// arbitrary browser-provided object key, host, bucket or version.
#[async_trait]
pub trait PrivateObjectDeliveryPort: Send + Sync {
    async fn grant_verified_object(
        &self,
        object: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError>;
}
