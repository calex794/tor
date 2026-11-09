//! Encode a public profile segment for notification links.
use std::fmt::Write;

/// Encode a single public profile path segment, not an untrusted redirect.
pub(super) fn encode_segment(name: &str) -> String {
    let mut result = String::new();
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("write to string");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::encode_segment;

    #[test]
    fn profile_notification_link_is_one_encoded_path_segment() {
        assert_eq!(
            encode_segment("Алиса / Боб"),
            "%D0%90%D0%BB%D0%B8%D1%81%D0%B0%20%2F%20%D0%91%D0%BE%D0%B1"
        );
        assert_eq!(encode_segment("//evil.example"), "%2F%2Fevil.example");
    }
}
