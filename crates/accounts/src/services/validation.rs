//! Input invariants for web registration and login, independent of HTTP and SQL.
#![forbid(unsafe_code)]

use crate::domain::normalize_email;

pub struct NewAccount {
    pub email: String,
    pub display_name: String,
}

pub fn registration(
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<NewAccount, &'static str> {
    let email = normalize_email(email)?;
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
    if new_password != confirmation {
        return Err("Подтверждение нового пароля не совпадает");
    }
    if !(12..=128).contains(&new_password.chars().count()) || new_password.len() > 512 {
        return Err("Новый пароль должен содержать от 12 до 128 символов");
    }
    if new_password == current {
        return Err("Новый пароль должен отличаться от текущего");
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
    fn password_change_preserves_legacy_current_and_checks_new_policy() {
        assert!(password_change("old", "новый-длинный-пароль", "новый-длинный-пароль").is_ok());
        assert!(password_change("", "новый-длинный-пароль", "новый-длинный-пароль").is_err());
        assert!(password_change("old", "short", "short").is_err());
        assert!(password_change("old", "новый-длинный-пароль", "другой-пароль").is_err());
        assert!(password_change("тот же длинный пароль", "тот же длинный пароль", "тот же длинный пароль").is_err());
    }

    #[test]
    fn login_limits_input_but_does_not_reject_existing_short_passwords() {
        assert_eq!(login("A@B", "legacy").unwrap(), "a@b");
        assert!(login("A@B", "").is_err());
        assert!(login("A@B", &"x".repeat(513)).is_err());
    }
}
