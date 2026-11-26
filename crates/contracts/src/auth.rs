//! A/I: reuse the established actor and positive ID; no second identity stack.
pub use crate::paid_delivery::{Actor, PositiveId as UserId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRole {
    Author,
    Moderator,
    Administrator,
}
impl AccountRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Moderator => "moderator",
            Self::Administrator => "administrator",
        }
    }
}

/// Produced by the account session port, never deserialized from browser claims.
/// Global roles carry no book membership or reading/purchase entitlement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionActor {
    pub actor: Actor,
    pub author: bool,
    pub moderator: bool,
    pub administrator: bool,
}

/// One-use proof for saving the singleton encrypted S3 profile, not activation.
pub const S3_CONNECTION_STEP_UP_SCOPE: &str = "storage.s3.connection.save.v1";
