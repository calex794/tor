//! Explicitly nonfinancial campaigns; dates are UTC instants, not browser time.
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use tor_contracts::exposure::PromotionSlot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorialError {
    Invalid,
    Denied,
    NotFound,
    Conflict,
    Unavailable,
}

/// Verified again against A users, session and role inside every mutation.
#[derive(Clone)]
pub struct Administrator {
    pub user_id: i64,
    pub session_digest: [u8; 32],
}

/// Optional A identity used only for this response's live access and explicit Hide.
/// This value is neither persisted nor supplied to the anonymous observation port.
#[derive(Clone)]
pub struct Viewer {
    pub user_id: i64,
    pub session_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Update,
    Activate,
    Pause,
    End,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Activate => "activate",
            Self::Pause => "pause",
            Self::End => "end",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignSpec {
    pub book_id: i64,
    pub slot: PromotionSlot,
    pub starts_at: String,
    pub ends_at: String,
    pub priority: i16,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NormalizedSpec {
    pub(crate) book_id: i64,
    pub(crate) slot: PromotionSlot,
    pub(crate) starts_at: i64,
    pub(crate) ends_at: i64,
    pub(crate) priority: i16,
}
impl CampaignSpec {
    fn normalize(&self) -> Result<NormalizedSpec, EditorialError> {
        fn date(value: &str) -> Result<i64, EditorialError> {
            if value.len() > 40 {
                return Err(EditorialError::Invalid);
            }
            let time = DateTime::parse_from_rfc3339(value).map_err(|_| EditorialError::Invalid)?;
            if time.timestamp_subsec_nanos() != 0 {
                return Err(EditorialError::Invalid);
            }
            Ok(time.timestamp())
        }
        let starts_at = date(&self.starts_at)?;
        let ends_at = date(&self.ends_at)?;
        if self.book_id <= 0
            || !(0..=100).contains(&self.priority)
            || starts_at < 0
            || ends_at > 32_503_680_000
            || !(1..=366 * 86_400).contains(&(ends_at - starts_at))
        {
            return Err(EditorialError::Invalid);
        }
        Ok(NormalizedSpec {
            book_id: self.book_id,
            slot: self.slot,
            starts_at,
            ends_at,
            priority: self.priority,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Command {
    pub action: Action,
    pub campaign_id: Option<i64>,
    pub expected_revision: i64,
    pub details: Option<CampaignSpec>,
}
#[derive(Clone, Debug, Serialize)]
pub struct NormalizedCommand {
    pub(crate) action: Action,
    pub(crate) campaign_id: Option<i64>,
    pub(crate) expected_revision: i64,
    pub(crate) details: Option<NormalizedSpec>,
}
impl Command {
    pub fn normalize(&self) -> Result<NormalizedCommand, EditorialError> {
        let valid_id = self.campaign_id.is_some_and(|id| id > 0);
        let valid = match self.action {
            Action::Create => self.campaign_id.is_none() && self.expected_revision == 0,
            _ => valid_id && (1..i64::MAX).contains(&self.expected_revision),
        };
        if !valid
            || matches!(self.action, Action::Create | Action::Update) != self.details.is_some()
        {
            return Err(EditorialError::Invalid);
        }
        Ok(NormalizedCommand {
            action: self.action,
            campaign_id: self.campaign_id,
            expected_revision: self.expected_revision,
            details: self
                .details
                .as_ref()
                .map(CampaignSpec::normalize)
                .transpose()?,
        })
    }
}
pub fn valid_command_key(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub campaign_id: i64,
    pub revision: i64,
}
#[derive(Clone, Debug)]
pub struct Campaign {
    pub id: i64,
    pub book_id: i64,
    pub title: String,
    pub slot: PromotionSlot,
    pub status: String,
    pub starts_at: String,
    pub ends_at: String,
    pub priority: i16,
    pub revision: i64,
    pub discoverable: bool,
}
#[derive(Clone, Debug)]
pub struct CampaignPage {
    pub items: Vec<Campaign>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct EditorialCard {
    pub campaign_id: i64,
    pub revision: i64,
    pub book_id: i64,
    pub title: String,
    pub slot: PromotionSlot,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> CampaignSpec {
        CampaignSpec {
            book_id: 4,
            slot: PromotionSlot::HomeFeatured,
            starts_at: "2026-11-11T00:00:00Z".into(),
            ends_at: "2026-11-12T00:00:00Z".into(),
            priority: 1,
        }
    }
    #[test]
    fn schedule_normalizes_instants_and_rejects_ambiguous_or_unbounded_values() {
        let original = spec();
        let mut equivalent = spec();
        equivalent.starts_at = "2026-11-11T05:00:00+05:00".into();
        assert_eq!(original.normalize(), equivalent.normalize());
        for bad in ["2026-11-11T00:00:00", "2026-11-11T00:00:00.1Z", "garbage"] {
            equivalent.starts_at = bad.into();
            assert_eq!(equivalent.normalize(), Err(EditorialError::Invalid));
        }
        equivalent = spec();
        equivalent.ends_at = equivalent.starts_at.clone();
        assert_eq!(equivalent.normalize(), Err(EditorialError::Invalid));
        equivalent = spec();
        equivalent.ends_at = "2028-01-01T00:00:00Z".into();
        assert_eq!(equivalent.normalize(), Err(EditorialError::Invalid));
    }
    #[test]
    fn commands_are_typed_and_do_not_accept_money_or_paid_slots() {
        let create = Command {
            action: Action::Create,
            campaign_id: None,
            expected_revision: 0,
            details: Some(spec()),
        };
        assert!(create.normalize().is_ok());
        assert!(Command {
            action: Action::Activate,
            ..create
        }
        .normalize()
        .is_err());
        assert!("catalog_listing".parse::<PromotionSlot>().is_err());
        assert!("search_results".parse::<PromotionSlot>().is_err());
        assert!(valid_command_key(&"a".repeat(43)));
        assert!(!valid_command_key(&"a".repeat(42)));
    }
}
