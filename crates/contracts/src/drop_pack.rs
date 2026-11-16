//! DROP R4a neutral B/G data contract: an exact physical placement descriptor.
//!
//! G must issue this only AFTER an independent full S3 GET and content proof.
//! The struct is public and serializable-by-value for crate isolation: it is
//! NOT a cryptographic attestation and MUST NOT be accepted from an HTTP actor.
//! B authorizes all editions and fences the job before storing candidates.

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PackPlacementTarget {
    pub bucket: String,
    pub pack_key: String,
    pub manifest_key: String,
    pub pack_sha256: [u8; 32],
    pub manifest_sha256: [u8; 32],
    pub pack_bytes: u64,
    pub manifest_bytes: u64,
    pub seek_table_start: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PackPlacementFrame {
    pub chapter_id: i64,
    pub version_id: i64,
    pub content_sha256: [u8; 32],
    pub size_bytes: u64,
    pub frame_offset: u64,
    pub frame_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PackPlacementReceipt {
    pub target: PackPlacementTarget,
    /// Ordered physical frames; MUST be compared to a preapproved B inventory.
    pub frames: Vec<PackPlacementFrame>,
}
