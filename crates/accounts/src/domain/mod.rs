//! Identity value normalization, independent of storage and HTTP.
#![forbid(unsafe_code)]

/// Single canonical email key used for uniqueness, login, and account lookup.
/// Deliberately conservative: provider-specific alias rewriting is not allowed.
pub fn normalize_email(raw: &str) -> Result<String, &'static str> {
    let email = raw.trim().to_lowercase();
    if email.len() > 254
        || email.len() < 3
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("Некорректный адрес почты");
    }
    let (local, domain) = email.split_once('@').ok_or("Некорректный адрес почты")?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err("Некорректный адрес почты");
    }
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_case_and_surrounding_spaces_do_not_create_an_alias() {
        assert_eq!(
            normalize_email("  Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        assert_eq!(
            normalize_email("ALICE@example.com").unwrap(),
            "alice@example.com"
        );
    }

    #[test]
    fn rejects_malformed_emails_without_rewriting_provider_aliases() {
        for invalid in [
            "",
            "@host",
            "name@",
            "name@@host",
            "bad name@host",
            "a\n@host",
        ] {
            assert!(normalize_email(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(
            normalize_email("alice+example@example.com").unwrap(),
            "alice+example@example.com"
        );
    }
}
