//! S050/S051: support identity never shares a private-message or comment ACL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Requester {
    Guest([u8; 32]),
    User { id: i64, guest: Option<[u8; 32]> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportError {
    Invalid,
    NotFound,
    Denied,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chat {
    pub id: i64,
    pub requester_id: Option<i64>,
    pub guest_alias: String,
    pub last_message_id: Option<i64>,
    pub last_message_preview: String,
    pub unread: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: i64,
    pub sender_kind: String,
    pub body: String,
    pub created_at: String,
}

pub struct MessagePage {
    pub chat: Chat,
    pub messages: Vec<Message>,
    pub has_more: bool,
}

pub fn clean_body(body: &str) -> Result<String, SupportError> {
    let body = body.replace("\r\n", "\n").replace('\r', "\n");
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 4000 || body.contains('\0') {
        return Err(SupportError::Invalid);
    }
    Ok(body.to_owned())
}

/// The legacy HTML key is optional, trimmed and bounded to 128 Unicode chars.
pub fn client_key(key: Option<&str>) -> Option<String> {
    key.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(128).collect())
}

pub fn validate_requester(who: &Requester) -> Result<(), SupportError> {
    if matches!(who, Requester::User { id, .. } if *id <= 0) {
        return Err(SupportError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_11_support_message_limits_count_unicode_and_preserve_plain_text() {
        assert_eq!(clean_body(" \r\nтекст\r🐼 ").unwrap(), "текст\n🐼");
        assert!(clean_body(&"🐼".repeat(4000)).is_ok());
        assert_eq!(clean_body(&"🐼".repeat(4001)), Err(SupportError::Invalid));
        assert_eq!(clean_body(" \n "), Err(SupportError::Invalid));
        assert_eq!(clean_body("a\0b"), Err(SupportError::Invalid));
        assert_eq!(
            client_key(Some(&"я".repeat(129))).unwrap().chars().count(),
            128
        );
    }
}
