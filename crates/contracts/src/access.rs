//! ACL-01: transport- and storage-independent authorization decisions.
//!
//! Callers MUST populate facts from trusted, actor-scoped PostgreSQL ports.
//! This module never accepts browser claims as entitlements, and never
//! authorizes a signed object or a purchase on its own.
use crate::paid_delivery::PositiveId;

/// Book project membership. Not a reading subscription or team role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookRole {
    Owner,
    Translator,
    Moderator,
}

/// An active member may work on a book; project ownership is more specific.
/// A separate author-preview gate must still check chapter holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookMembership {
    pub role: BookRole,
    pub active: bool,
}

impl BookMembership {
    pub fn can_manage(self) -> bool {
        self.active
    }

    pub fn can_transfer_owner(self) -> bool {
        self.active && matches!(self.role, BookRole::Owner)
    }
}

/// Organization roles are not book project roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeamRole {
    Owner,
    Manager,
    Member,
}

/// Team-derived read authority is a revocable intersection of three live facts.
/// It does NOT create a book editor role or an immutable purchase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TeamReadingMembership {
    pub team_active: bool,
    pub membership_active: bool,
    pub book_link_active: bool,
}

impl TeamReadingMembership {
    pub fn permits_current(self) -> bool {
        self.team_active && self.membership_active && self.book_link_active
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessDenial {
    HardBlocked,
    Locked,
}

/// All version IDs must be resolved server-side. A direct purchase is pinned
/// to the purchased version; Layer/team/free rights resolve current published
/// versions, not history and never an arbitrary client-chosen version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadGrant {
    DirectPurchasePinned { version_id: PositiveId },
    LayerCurrent { version_id: PositiveId },
    TeamCurrent { version_id: PositiveId },
    PublicFreeCurrent { version_id: PositiveId },
}

/// Facts must already be actor-scoped and verified by their owning services.
/// In particular, revoked/refunded purchases MUST NOT appear in
/// direct_purchase_pinned_version. Free/current IDs must be published/readable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VerifiedReaderFacts {
    pub hard_blocked: bool,
    pub direct_purchase_pinned_version: Option<PositiveId>,
    pub layer_current_version: Option<PositiveId>,
    pub team_current_version: Option<PositiveId>,
    pub public_free_current_version: Option<PositiveId>,
}

/// Hard legal/admin/deletion blocks beat every ordinary reader entitlement.
/// Purchase remains independent of team membership, and always selects the
/// immutable pinned version. No permission exists merely from a role label.
pub fn decide_read(facts: VerifiedReaderFacts) -> Result<ReadGrant, AccessDenial> {
    if facts.hard_blocked {
        return Err(AccessDenial::HardBlocked);
    }
    if let Some(version_id) = facts.direct_purchase_pinned_version {
        return Ok(ReadGrant::DirectPurchasePinned { version_id });
    }
    if let Some(version_id) = facts.layer_current_version {
        return Ok(ReadGrant::LayerCurrent { version_id });
    }
    if let Some(version_id) = facts.team_current_version {
        return Ok(ReadGrant::TeamCurrent { version_id });
    }
    if let Some(version_id) = facts.public_free_current_version {
        return Ok(ReadGrant::PublicFreeCurrent { version_id });
    }
    Err(AccessDenial::Locked)
}
