#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailError {
    Invalid,
    NotFound,
    Conflict,
    Blocked,
    RateLimited,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: i64,
    pub sender_id: i64,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Conversation {
    pub id: i64,
    pub peer_id: i64,
    pub unread: i64,
    pub blocked: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct MessagePage {
    pub items: Vec<Message>,
    pub peer_id: i64,
    pub blocked: bool,
    pub blocked_by_me: bool,
    pub has_more: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SentMessage {
    pub id: i64,
    pub replayed: bool,
}

pub fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Technical TOR limit, not a claim that the donor used this exact bound.
pub fn clean_body(body: &str) -> Result<&str, MailError> {
    let body = body.trim();
    if body.is_empty()
        || body.chars().count() > 2000
        || body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
    {
        return Err(MailError::Invalid);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_08_bounds_unicode_and_command_identity() {
        assert_eq!(clean_body("  Привет 🐼\n  "), Ok("Привет 🐼"));
        assert!(clean_body("\0").is_err());
        assert!(clean_body("  ").is_err());
        assert!(clean_body(&"я".repeat(2001)).is_err());
        assert!(clean_body(&"🐼".repeat(2000)).is_ok());
        assert!(valid_key(&"A".repeat(43)));
        assert!(!valid_key(&"/".repeat(43)));
    }
}
