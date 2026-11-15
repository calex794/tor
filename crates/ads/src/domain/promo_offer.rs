//! P1b: provider-neutral offer intent, with a strict external Boosty URL boundary.
//! No money, purchase evidence, user history or active serving lives in this module.
use chrono::DateTime;
use std::fmt;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferError {
    InvalidTarget,
    InvalidDestination,
    InvalidCreative,
    InvalidSchedule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalKind {
    Discount,
    Trial,
    Post,
    Bundle,
}

/// The link may carry a reusable promotion token. It must never be logged
/// by a derived Debug implementation or mistaken for a payment receipt.
#[derive(Clone, Eq, PartialEq)]
pub struct BoostyUrl(String);

impl fmt::Debug for BoostyUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoostyUrl([redacted])")
    }
}

impl BoostyUrl {
    pub fn parse(value: &str) -> Result<Self, OfferError> {
        if value.is_empty()
            || value.len() > 2048
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(OfferError::InvalidDestination);
        }
        let url = Url::parse(value).map_err(|_| OfferError::InvalidDestination)?;
        if url.scheme() != "https"
            || url.host_str() != Some("boosty.to")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
            || url.cannot_be_a_base()
            || url
                .path_segments()
                .and_then(|mut p| p.next())
                .is_none_or(str::is_empty)
        {
            return Err(OfferError::InvalidDestination);
        }
        Ok(Self(url.to_string()))
    }

    /// Exposed only at trusted outbound-link rendering; no analytics payload
    /// is allowed to contain the full destination.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromoTarget {
    Book(i64),
    Package(i64),
    Boosty {
        kind: ExternalKind,
        destination: BoostyUrl,
    },
}

impl PromoTarget {
    pub fn validate(&self) -> Result<(), OfferError> {
        match self {
            Self::Book(id) | Self::Package(id) if *id <= 0 => Err(OfferError::InvalidTarget),
            _ => Ok(()),
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(self, Self::Boosty { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromoAudience {
    Everyone,
    Guests,
    Registered,
    ConfirmedChapterBuyers,
}

/// These selectors only describe existing canonical account/commerce facts.
/// No "Boosty subscriber" predicate exists without verified provider evidence.
impl PromoAudience {
    pub const ALL: [Self; 4] = [
        Self::Everyone,
        Self::Guests,
        Self::Registered,
        Self::ConfirmedChapterBuyers,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromoCreative {
    pub title: String,
    pub subtitle: String,
    pub cta_label: String,
    pub terms_snapshot: String,
}

fn text_valid(value: &str, maximum: usize, required: bool, multiline: bool) -> bool {
    let count = value.chars().count();
    if count > maximum || (required && (count == 0 || value.trim().is_empty())) {
        return false;
    }
    value
        .chars()
        .all(|character| !character.is_control() || (multiline && character == '\n'))
}

impl PromoCreative {
    pub fn validate(&self) -> Result<(), OfferError> {
        if !text_valid(&self.title, 100, true, false)
            || !text_valid(&self.subtitle, 220, false, false)
            || !text_valid(&self.cta_label, 40, true, false)
            || !text_valid(&self.terms_snapshot, 600, true, true)
        {
            return Err(OfferError::InvalidCreative);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromoDraft {
    pub target: PromoTarget,
    pub audience: PromoAudience,
    pub creative: PromoCreative,
    pub starts_at: String,
    pub ends_at: String,
    pub priority: i16,
}

impl PromoDraft {
    pub fn validate(&self) -> Result<(), OfferError> {
        fn seconds(value: &str) -> Result<i64, OfferError> {
            if value.len() > 40 {
                return Err(OfferError::InvalidSchedule);
            }
            let parsed =
                DateTime::parse_from_rfc3339(value).map_err(|_| OfferError::InvalidSchedule)?;
            if parsed.timestamp_subsec_nanos() != 0 {
                return Err(OfferError::InvalidSchedule);
            }
            Ok(parsed.timestamp())
        }
        self.target.validate()?;
        self.creative.validate()?;
        let start = seconds(&self.starts_at)?;
        let end = seconds(&self.ends_at)?;
        if start < 0
            || end > 32_503_680_000
            || !(0..=100).contains(&self.priority)
            || !matches!(end.checked_sub(start), Some(1..=31_622_400))
        {
            return Err(OfferError::InvalidSchedule);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> PromoDraft {
        PromoDraft {
            target: PromoTarget::Boosty {
                kind: ExternalKind::Trial,
                destination: BoostyUrl::parse("https://boosty.to/author?offer=opaque_token")
                    .unwrap(),
            },
            audience: PromoAudience::Everyone,
            creative: PromoCreative {
                title: "Пробный период".into(),
                subtitle: "Условия определяет Boosty".into(),
                cta_label: "Проверить предложение".into(),
                terms_snapshot: "Актуальность и доступность подтверждает Boosty.".into(),
            },
            starts_at: "2026-11-15T00:00:00Z".into(),
            ends_at: "2026-11-16T00:00:00Z".into(),
            priority: 20,
        }
    }

    #[test]
    fn external_urls_accept_only_canonical_https_boosty_targets() {
        for good in [
            "https://boosty.to/author",
            "https://boosty.to/author/posts/1234",
            "https://boosty.to/author?offer=token_123",
        ] {
            assert_eq!(BoostyUrl::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "http://boosty.to/author",
            "https://boosty.to.evil.invalid/author",
            "https://evil.invalid/boosty.to/author",
            "https://user:password@boosty.to/author",
            "https://boosty.to:8443/author",
            "https://boosty.to/author#fragment",
            "https://boosty.to/",
            "javascript:alert(1)",
            "https://boosty.to/author\n",
        ] {
            assert_eq!(
                BoostyUrl::parse(bad),
                Err(OfferError::InvalidDestination),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn outbound_url_debug_cannot_leak_promo_query_or_userinfo() {
        let link = BoostyUrl::parse("https://boosty.to/author?trial=private_code").unwrap();
        let debug = format!("{link:?}");
        assert!(!debug.contains("private_code"));
        assert!(!debug.contains("author"));
    }

    #[test]
    fn draft_is_nonfinancial_bounded_and_typed() {
        let valid = draft();
        assert_eq!(valid.validate(), Ok(()));
        assert!(valid.target.is_external());
        assert_eq!(PromoAudience::ALL.len(), 4);
        let mut invalid = draft();
        invalid.target = PromoTarget::Book(0);
        assert_eq!(invalid.validate(), Err(OfferError::InvalidTarget));
        invalid = draft();
        invalid.target = PromoTarget::Package(-1);
        assert_eq!(invalid.validate(), Err(OfferError::InvalidTarget));
        invalid = draft();
        invalid.creative.cta_label = "x".repeat(41);
        assert_eq!(invalid.validate(), Err(OfferError::InvalidCreative));
        invalid = draft();
        invalid.ends_at = invalid.starts_at.clone();
        assert_eq!(invalid.validate(), Err(OfferError::InvalidSchedule));
        invalid = draft();
        invalid.ends_at = "2028-02-01T00:00:00Z".into();
        assert_eq!(invalid.validate(), Err(OfferError::InvalidSchedule));
        invalid = draft();
        invalid.priority = 101;
        assert_eq!(invalid.validate(), Err(OfferError::InvalidSchedule));
        invalid = draft();
        invalid.starts_at = "2026-11-15T00:00:00".into();
        assert_eq!(invalid.validate(), Err(OfferError::InvalidSchedule));
        invalid = draft();
        invalid.creative.title = "<script>".into();
        assert_eq!(invalid.validate(), Ok(()), "text is escaped by trusted SSR");
    }
}
