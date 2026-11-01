//! A/I: reuse the established actor and positive ID; no second identity stack.
pub use crate::paid_delivery::{Actor, PositiveId as UserId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRole {
    Author,
    Administrator,
}
impl AccountRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
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
    pub administrator: bool,
}
