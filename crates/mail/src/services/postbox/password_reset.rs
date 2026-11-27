//! Password recovery MIME composition, using only the shared Postbox adapter.
use super::{mailbox, PostboxTransport, PrepareFailure, PreparedPostboxMessage};
use zeroize::Zeroizing;

/// Operator-selected HTTPS origin, never an address supplied by a visitor.
/// Reject paths, userinfo, query, fragments and non-DNS destinations.
pub struct ResetOrigin(String);

impl ResetOrigin {
    pub fn new(origin: &str) -> Option<Self> {
        let authority = origin.strip_prefix("https://")?;
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => {
                if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                (host, Some(port.parse::<u16>().ok()?))
            }
            None => (authority, None),
        };
        if port == Some(0) || mailbox(&format!("reset@{host}")).is_none() {
            return None;
        }
        Some(Self(origin.to_owned()))
    }
}

fn body(origin: &ResetOrigin, token: &str) -> Option<Zeroizing<String>> {
    if token.len() != 43
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return None;
    }
    Some(Zeroizing::new(format!(
        "Вы запросили восстановление пароля NEwBOO.\n\n\
         Чтобы установить новый пароль, откройте ссылку (действует 30 минут):\n\
         {}/reset-password#{}\n\n\
         Если вы не запрашивали восстановление, проигнорируйте письмо.\n\
         Пароль не изменится, пока вы не подтвердите его самостоятельно.\n",
        origin.0, token
    )))
}

impl PostboxTransport {
    /// Called only with a fresh in-memory A-07 secret after the common final
    /// admission transaction committed its digest and correlation marker.
    /// No account lookup, quota reservation, secret storage or submission.
    pub fn prepare_reset_message(
        &self,
        origin: &ResetOrigin,
        recipient: &str,
        token: &str,
        marker: &str,
    ) -> Result<PreparedPostboxMessage, PrepareFailure> {
        let body = body(origin, token).ok_or(PrepareFailure::InvalidResetProof)?;
        self.prepare(
            recipient,
            "NEwBOO — восстановление пароля",
            body.as_str(),
            marker,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_configured_https_origin_and_fragment_proof_are_rendered() {
        for origin in [
            "",
            "http://books.example",
            "https://books.example/login",
            "https://books.example?next=evil",
            "https://books.example#fragment",
            "https://books.example@evil.example",
            "https://books.example:0",
            "https://127.0.0.1",
            "https://books.example\r\nBcc:evil@example.com",
        ] {
            assert!(ResetOrigin::new(origin).is_none());
        }
        let origin = ResetOrigin::new("https://books.example:443").unwrap();
        let token = "A".repeat(43);
        let rendered = body(&origin, &token).unwrap();
        assert!(rendered.contains(&format!("https://books.example:443/reset-password#{token}")));
        assert!(!rendered.contains('?'));
        assert_eq!(rendered.matches(&token).count(), 1);
        assert!(rendered.contains("30 минут"));
        for invalid in ["short", &format!("{}\n", "A".repeat(42)), &"A".repeat(44)] {
            assert!(body(&origin, invalid).is_none());
        }
    }
}
