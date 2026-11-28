//! Shared email and handle login identifiers, without a second account authority.
use super::{profile::normalize_handle, validation::login};

/// Disjoint namespaces: existing email login keeps the legacy validation
/// contract; handles use exactly the same normalization as profile settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoginIdentity {
    Email(String),
    Handle(String),
}

impl LoginIdentity {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Email(value) | Self::Handle(value) => value,
        }
    }
}

pub fn login_identity(raw: &str, password: &str) -> Result<LoginIdentity, &'static str> {
    if raw.contains('@') {
        return login(raw, password).map(LoginIdentity::Email);
    }
    if password.is_empty() || password.len() > 512 {
        return Err("Некорректные учётные данные");
    }
    let handle = normalize_handle(raw)?;
    if handle.is_empty() {
        return Err("Некорректные учётные данные");
    }
    Ok(LoginIdentity::Handle(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_identity_keeps_legacy_email_and_disjoint_handle_namespace() {
        assert_eq!(
            login_identity(" A@B ", "legacy").unwrap(),
            LoginIdentity::Email("a@b".into())
        );
        assert_eq!(
            login_identity(" A.B_1 ", "legacy").unwrap(),
            LoginIdentity::Handle("a.b_1".into())
        );
        assert!(login_identity("reader", "").is_err());
        assert!(login_identity("root", "legacy").is_err());
        assert!(login_identity("bad@handle@", "legacy").is_err());
    }
}
