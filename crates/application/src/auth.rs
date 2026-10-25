//! Input invariants for web registration and login, independent of HTTP and SQL.
#![forbid(unsafe_code)]

use bokarust_domain::accounts::normalize_email;

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
    fn login_limits_input_but_does_not_reject_existing_short_passwords() {
        assert_eq!(login("A@B", "legacy").unwrap(), "a@b");
        assert!(login("A@B", "").is_err());
        assert!(login("A@B", &"x".repeat(513)).is_err());
    }
}
