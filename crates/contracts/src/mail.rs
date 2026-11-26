//! MAIL-01: trusted notification references, not arbitrary outbound messages.
use crate::auth::UserId;
use crate::paid_delivery::PositiveId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailPurpose {
    RegistrationConfirmation,
    SupportReply,
    ProjectNews,
}

impl MailPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegistrationConfirmation => "registration_confirmation",
            Self::SupportReply => "support_reply",
            Self::ProjectNews => "project_news",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "registration_confirmation" => Some(Self::RegistrationConfirmation),
            "support_reply" => Some(Self::SupportReply),
            "project_news" => Some(Self::ProjectNews),
            _ => None,
        }
    }
}

/// Internal event reference. The delivery worker must resolve current account,
/// source ownership and (for news) subscription before rendering or sending.
/// No email address, verification token, ticket body or SMTP secret is queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MailIntent {
    pub purpose: MailPurpose,
    pub event_id: PositiveId,
    pub recipient: UserId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_three_notification_purposes_are_supported() {
        for purpose in [
            MailPurpose::RegistrationConfirmation,
            MailPurpose::SupportReply,
            MailPurpose::ProjectNews,
        ] {
            assert_eq!(MailPurpose::parse(purpose.as_str()), Some(purpose));
        }
        assert_eq!(MailPurpose::parse("login_code"), None);
        assert_eq!(MailPurpose::parse("arbitrary_message"), None);
    }
}
