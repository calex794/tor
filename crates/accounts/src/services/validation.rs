//! Input invariants for web registration and login, independent of HTTP and SQL.
#![forbid(unsafe_code)]

use super::profile::normalize_handle;
use crate::domain::{email::registration_email, normalize_email};

pub struct NewAccount {
    pub email: String,
    pub display_name: String,
}

pub fn registration(
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<NewAccount, &'static str> {
    let email = registration_email(email)?;
    let display_name = display_name.trim();
    let length = display_name.chars().count();
    if !(1..=80).contains(&length) || display_name.chars().any(char::is_control) {
        return Err("Имя должно содержать от 1 до 80 символов");
    }
    if !(12..=128).contains(&password.chars().count()) || password.len() > 512 {
        return Err("Пароль должен содержать от 12 до 128 символов");
    }
    Ok(NewAccount {
        email,
        display_name: display_name.to_owned(),
    })
}

/// A validated signup identity. Initial display name is the canonical handle;
/// the profile's display name remains separately editable.
pub struct NamedAccount {
    email: String,
    handle: String,
}

impl NamedAccount {
    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }
}

pub fn named_registration(
    email: &str,
    handle: &str,
    password: &str,
    confirmation: &str,
) -> Result<NamedAccount, &'static str> {
    let handle = normalize_handle(handle)?;
    if handle.is_empty() {
        return Err("Укажите логин");
    }
    if password != confirmation {
        return Err("Пароли не совпадают");
    }
    let account = registration(email, &handle, password)?;
    Ok(NamedAccount {
        email: account.email,
        handle,
    })
}

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

/// The current password may be shorter (legacy users); the new one must
/// meet the same policy as new registrations.
pub fn password_change(
    current: &str,
    new_password: &str,
    confirmation: &str,
) -> Result<(), &'static str> {
    if current.is_empty() || current.len() > 512 {
        return Err("Укажите текущий пароль");
    }
    password_reset(new_password, confirmation)?;
    if new_password == current {
        return Err("Новый пароль должен отличаться от текущего");
    }
    Ok(())
}

pub fn password_reset(new_password: &str, confirmation: &str) -> Result<(), &'static str> {
    if new_password != confirmation {
        return Err("Подтверждение нового пароля не совпадает");
    }
    if !(12..=128).contains(&new_password.chars().count()) || new_password.len() > 512 {
        return Err("Новый пароль должен содержать от 12 до 128 символов");
    }
    Ok(())
}

pub fn login(email: &str, password: &str) -> Result<String, &'static str> {
    let email = normalize_email(email)?;
    if password.is_empty() || password.len() > 512 {
        return Err("Некорректные учётные данные");
    }
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_validates_inputs_without_changing_password() {
        let account = registration("  USer@Example.COM ", "  Автор  ", "мой_надежный_пароль")
            .expect("valid registration");
        assert_eq!(account.email, "user@example.com");
        assert_eq!(account.display_name, "Автор");
        assert!(registration("invalid", "Автор", "мой_надежный_пароль").is_err());
        assert!(registration("a@b", "", "мой_надежный_пароль").is_err());
        assert!(registration("a@b", "A\nB", "мой_надежный_пароль").is_err());
        assert!(registration("a@b", "Автор", "short").is_err());
        assert!(registration("a@b", "Автор", &"x".repeat(129)).is_err());
    }

    #[test]
    fn registration_rejects_invalid_email_without_changing_legacy_login() {
        let password = "мой_надежный_пароль";
        for invalid in [
            "a@b",
            "user@gmail..com",
            "user@-gmail.com",
            "a..b@example.com",
        ] {
            assert!(registration(invalid, "Reader", password).is_err());
            assert!(login(invalid, password).is_ok());
        }
        assert!(registration("reader+books@example.com", "Reader", password).is_ok());
    }

    #[test]
    fn named_signup_requires_canonical_unique_key_and_exact_confirmation() {
        let password = " Valid password ";
        let account =
            named_registration(" Reader@Example.COM ", " A.B_1 ", password, password).unwrap();
        assert_eq!(account.email(), "reader@example.com");
        assert_eq!(account.handle(), "a.b_1");
        for bad in ["", "  ", "admin", "me", "a", "a@b", "русский", "a b"] {
            assert!(named_registration("reader@example.com", bad, password, password).is_err());
        }
        assert!(
            named_registration("reader@example.com", "reader", password, password.trim()).is_err()
        );
        assert!(named_registration("a@b", "reader", password, password).is_err());
        assert!(named_registration("reader@example.com", "reader", "short", "short").is_err());
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

    #[test]
    fn password_change_preserves_legacy_current_and_checks_new_policy() {
        assert!(password_change("old", "новый-длинный-пароль", "новый-длинный-пароль").is_ok());
        assert!(password_change("", "новый-длинный-пароль", "новый-длинный-пароль").is_err());
        assert!(password_change("old", "short", "short").is_err());
        assert!(password_change("old", "новый-длинный-пароль", "другой-пароль").is_err());
        assert!(password_change(
            "тот же длинный пароль",
            "тот же длинный пароль",
            "тот же длинный пароль"
        )
        .is_err());
    }

    #[test]
    fn login_limits_input_but_does_not_reject_existing_short_passwords() {
        assert_eq!(login("A@B", "legacy").unwrap(), "a@b");
        assert!(login("A@B", "").is_err());
        assert!(login("A@B", &"x".repeat(513)).is_err());
    }
}
