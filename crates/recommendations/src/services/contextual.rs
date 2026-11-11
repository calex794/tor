//! Versioned metadata/manual model. No history, quality attribution or audience assignment.
use crate::domain::feedback::FeedbackError;
use serde::{Deserialize, Serialize};

pub const MODEL: &str = "metadata_manual_v1";
pub const BUILD_PAGE: i64 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Refresh,
    Activate,
    Hold,
    Rollback,
    Cancel,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub action: Action,
    pub expected_revision: i64,
    pub generation: Option<String>,
}
impl Control {
    pub fn validate(&self) -> Result<(), FeedbackError> {
        super::feedback::validate_revision(self.expected_revision)?;
        match (self.action, self.generation.as_deref()) {
            (Action::Activate | Action::Cancel, Some(id)) if valid_id(id) => Ok(()),
            (Action::Refresh | Action::Hold | Action::Rollback, None) => Ok(()),
            _ => Err(FeedbackError::Invalid),
        }
    }
}
fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
#[derive(Debug, Serialize)]
pub struct Generation {
    pub id: String,
    pub status: String,
    pub cursor_book_id: String,
    pub created_at: String,
    pub finished_at: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Status {
    pub model_version: &'static str,
    pub revision: i64,
    pub active_generation: Option<String>,
    pub previous_generation: Option<String>,
    pub held: bool,
    pub generations: Vec<Generation>,
}
#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct Receipt {
    pub revision: i64,
    pub generation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_have_no_rollout_or_personal_identifier() {
        for action in [Action::Refresh, Action::Hold, Action::Rollback] {
            assert!(Control {
                action,
                expected_revision: 0,
                generation: None
            }
            .validate()
            .is_ok());
            assert!(Control {
                action,
                expected_revision: 0,
                generation: Some("x".into())
            }
            .validate()
            .is_err());
        }
        assert!(Control {
            action: Action::Activate,
            expected_revision: 0,
            generation: None
        }
        .validate()
        .is_err());
        assert!(Control {
            action: Action::Activate,
            expected_revision: 0,
            generation: Some("00000000-0000-0000-0000-000000000001".into())
        }
        .validate()
        .is_ok());
        assert!(!valid_id("------------------------------------"));
        assert!(!valid_id("a"));
        assert!(!valid_id("00000000-0000-0000-0000-00000000000z"));
    }
}
