//! Referral rules shared by HTTP and PostgreSQL without framework coupling.
#![forbid(unsafe_code)]

pub const CAPTURE_DAYS: i64 = 30;
pub const CREATOR_ATTRIBUTION_DAYS: i64 = 365;

pub fn valid_code(raw: &str) -> bool {
    (6..=64).contains(&raw.len())
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A public link alias is lowercase ASCII, bounded, and not a reserved route.
pub fn valid_alias(alias: &str) -> bool {
    (3..=64).contains(&alias.len())
        && !["admin", "api", "auth", "books", "login", "mail", "me"].contains(&alias)
        && alias.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase() || byte.is_ascii_digit()
            } else {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
            }
        })
}

pub fn manual_code(raw: Option<&str>) -> Result<Option<&str>, &'static str> {
    let value = raw.unwrap_or_default().trim();
    if value.is_empty() {
        Ok(None)
    } else if valid_code(value) {
        Ok(Some(value))
    } else {
        Err("Некорректный код приглашения")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    ReaderSpend,
    CreatorIncome,
}

impl Program {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReaderSpend => "reader_spend",
            Self::CreatorIncome => "creator_income",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_09_codes_are_bounded_without_truncation_or_unicode_ambiguity() {
        assert!(valid_code("aB_012-abc"));
        assert!(!valid_code("five"));
        assert!(!valid_code(&"x".repeat(65)));
        assert!(!valid_code("ref%2Fcode"));
        assert!(!valid_code("реферал"));
        assert!(!valid_code("code with spaces"));
        assert_eq!(manual_code(Some(" abCD_123 ")).unwrap(), Some("abCD_123"));
        assert_eq!(manual_code(Some(" ")).unwrap(), None);
        assert!(manual_code(Some("bad code")).is_err());
        assert!(valid_alias("sale-2026"));
        for invalid in [
            "ab",
            "Admin",
            "me",
            "a/b",
            "%2f",
            "a..b",
            "реферал",
            "a".repeat(65).as_str(),
        ] {
            assert!(!valid_alias(invalid), "invalid alias accepted: {invalid}");
        }
    }

    #[test]
    fn f_09_programs_are_not_payment_rights() {
        assert_ne!(Program::ReaderSpend, Program::CreatorIncome);
        assert_eq!(CAPTURE_DAYS, 30);
        assert_eq!(CREATOR_ATTRIBUTION_DAYS, 365);
    }
}
