//! First trusted dispatch source: system notification of an administrator reply.
//! No support conversation content, recipient input or arbitrary links.
use axum::http::Uri;
use tor_accounts::repositories::PgAccounts;
use tor_contracts::mail::{MailIntent, MailPurpose};
use tor_mail::services::dispatch::{DispatchSource, ResolvedMail, SourceFailure};
use tor_support::repositories::postgres::PgSupport;

pub struct SupportDispatchSource {
    accounts: PgAccounts,
    support: PgSupport,
    link: String,
}

impl SupportDispatchSource {
    /// The origin comes from trusted application configuration, not a request.
    /// No provider access, queue creation or background worker starts here.
    pub fn new(accounts: PgAccounts, origin: &str) -> Option<Self> {
        let link = support_link(origin)?;
        let support = PgSupport::new(accounts.pool().clone());
        Some(Self {
            accounts,
            support,
            link,
        })
    }
}

fn support_link(origin: &str) -> Option<String> {
    let uri: Uri = origin.parse().ok()?;
    let authority = uri.authority()?;
    if uri.scheme_str() != Some("https")
        || uri.path() != "/"
        || uri.query().is_some()
        || authority.as_str().contains('@')
        || origin.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    Some(format!("https://{authority}/support"))
}

fn notification_body(link: &str) -> String {
    format!(
        "На ваше обращение в службу поддержки поступил ответ.\n\n\
         Чтобы прочитать его, войдите в свою учётную запись:\n{link}"
    )
}

impl DispatchSource for SupportDispatchSource {
    async fn resolve(&self, intent: MailIntent) -> Result<ResolvedMail, SourceFailure> {
        if intent.purpose != MailPurpose::SupportReply {
            // Registration and news have separate eligibility/token contracts.
            return Err(SourceFailure::Unavailable);
        }
        if !self
            .support
            .owns_mail_reply(intent)
            .await
            .map_err(|_| SourceFailure::Unavailable)?
        {
            return Err(SourceFailure::Cancelled);
        }
        let recipient = self
            .accounts
            .mail_delivery_address(intent.recipient)
            .await
            .map_err(|_| SourceFailure::Unavailable)?
            .ok_or(SourceFailure::Cancelled)?;
        Ok(ResolvedMail::new(
            intent,
            recipient,
            "Ответ службы поддержки".into(),
            notification_body(&self.link),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_notification_link_uses_only_configured_https_origin() {
        assert_eq!(
            support_link("https://pandaolate.ru/").as_deref(),
            Some("https://pandaolate.ru/support")
        );
        for invalid in [
            "http://pandaolate.ru",
            "https://pandaolate.ru/other",
            "https://pandaolate.ru/?x=1",
            "https://name@pandaolate.ru",
            "//pandaolate.ru",
        ] {
            assert!(support_link(invalid).is_none());
        }
        assert!(notification_body("https://pandaolate.ru/support")
            .contains("войдите в свою учётную запись"));
    }
}
