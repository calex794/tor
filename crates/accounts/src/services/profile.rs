//! A-06: settings validation shared by HTML and JSON adapters.
#[derive(Clone, Debug)]
pub struct ProfileUpdate {
    pub display_name: Option<String>,
    pub handle: Option<String>,
    pub adult_enabled: Option<bool>,
    pub expected_revision: i64,
}

impl ProfileUpdate {
    pub fn validate(&mut self) -> Result<(), &'static str> {
        if self.expected_revision < 0 {
            return Err("Некорректная версия настроек");
        }
        if let Some(name) = self.display_name.as_mut() {
            *name = name.trim().to_owned();
            if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
                return Err("Имя должно содержать от 1 до 80 символов");
            }
        }
        if let Some(handle) = self.handle.as_mut() {
            *handle = handle.trim().to_ascii_lowercase();
            if !handle.is_empty()
                && (!(3..=32).contains(&handle.len())
                    || !handle
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.".contains(&c))
                    || [
                        "admin",
                        "administrator",
                        "support",
                        "system",
                        "moderator",
                        "root",
                        "api",
                        "me",
                    ]
                    .contains(&handle.as_str()))
            {
                return Err("Адрес профиля: от 3 до 32 латинских букв, цифр, точек или подчёркиваний; служебные имена недоступны");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_06_normalizes_handle_and_rejects_reserved_unicode_and_control_names() {
        let mut update = ProfileUpdate {
            display_name: Some(" Reader ".into()),
            handle: Some(" A.B_1 ".into()),
            adult_enabled: None,
            expected_revision: 0,
        };
        update.validate().unwrap();
        assert_eq!(update.handle.as_deref(), Some("a.b_1"));
        for bad in ["admin", "root", "русский", "a", "a b", "ab/c"] {
            update.handle = Some(bad.into());
            assert!(update.validate().is_err());
        }
        update.handle = None;
        update.display_name = Some("bad\nname".into());
        assert!(update.validate().is_err());
    }
}
