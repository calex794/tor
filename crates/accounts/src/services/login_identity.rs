//! Shared email and handle login identifiers, without a second account authority.
use super::{profile::normalize_handle, security::session_token_digest, validation::login};

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

/// Preserve the original public per-submitted-identity key for all actions.
pub fn submitted_attempt_key(action: &str, identity: &str) -> [u8; 32] {
    session_token_digest(&format!("auth:{action}:identity:{identity}"))
}

/// Independent, private budget for an account, regardless of the presented
/// alias. Unknown identities have their own bounded, disjoint dummy budget.
pub(crate) fn account_attempt_key(user_id: Option<i64>, identity: &LoginIdentity) -> [u8; 32] {
    match user_id {
        Some(id) => session_token_digest(&format!("auth:login:account:{id}")),
        None => session_token_digest(&format!("auth:login:unresolved:{}", identity.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_account_budget_is_independent_of_alias() {
        let email = LoginIdentity::Email("reader@example.invalid".into());
        let handle = LoginIdentity::Handle("reader".into());
        let known = account_attempt_key(Some(42), &email);
        assert_eq!(known, account_attempt_key(Some(42), &handle));
        assert_ne!(known, account_attempt_key(Some(43), &handle));
        assert_ne!(known, account_attempt_key(None, &email));
        assert_ne!(
            account_attempt_key(None, &email),
            account_attempt_key(None, &handle)
        );
        assert_ne!(known, submitted_attempt_key("login", email.as_str()));
        assert_eq!(
            submitted_attempt_key("login", email.as_str()),
            session_token_digest("auth:login:identity:reader@example.invalid")
        );
    }

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
