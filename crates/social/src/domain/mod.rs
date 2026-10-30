#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailError {
    Invalid,
    NotFound,
    Conflict,
    Blocked,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: i64,
    pub sender_id: i64,
    pub body: String,
    pub created_at: String,
    pub created_at_display: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Conversation {
    pub id: i64,
    pub peer_id: i64,
    pub last_message_preview: String,
    pub last_message_at: String,
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

/// Source HTML keys intentionally differ from TOR JSON command keys.
pub fn valid_html_client_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':'))
}

/// Match the NEwBOO HTML/header helper: strip disallowed characters, truncate to 128.
pub fn clean_html_client_id(raw: &str) -> String {
    raw.trim().chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .take(128)
        .collect()
}

/// NEwBOO mail_service accepts nonempty stripped text without a 2000-character cap.
pub fn clean_body(body: &str) -> Result<String, MailError> {
    // HTML textarea submissions use CRLF; JSON/FormData retries may use LF.
    // Canonicalize transport line endings before bounds and replay comparison.
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let body = normalized.trim();
    if body.is_empty()
        || body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
    {
        return Err(MailError::Invalid);
    }
    Ok(body.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_08_source_nonempty_text_unicode_and_tor_command_identity() {
        assert_eq!(clean_body("  Привет 🐼\n  "), Ok("Привет 🐼".to_owned()));
        assert_eq!(clean_body("a\r\nb\rc"), clean_body("a\nb\nc"));
        assert!(clean_body("\0").is_err());
        assert!(clean_body("  ").is_err());
        assert!(clean_body(&"я".repeat(2001)).is_ok());
        assert!(clean_body(&"🐼".repeat(2000)).is_ok());
        assert!(valid_key(&"A".repeat(43)));
        assert!(!valid_key(&"/".repeat(43)));
        assert_eq!(clean_html_client_id("  id:abc/123!?  "), "id:abc123");
        assert_eq!(clean_html_client_id(&"a".repeat(140)).len(), 128);
        assert_eq!(clean_html_client_id("только Unicode"), "Unicode");
        assert!(valid_html_client_id("id:a-b.3"));
        assert!(!valid_html_client_id("unsafe/path"));
    }
}
