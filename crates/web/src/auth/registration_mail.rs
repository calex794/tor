//! Compose the trusted ingress, accounts mailbox policy and mail signer.
use std::net::IpAddr;
use tor_accounts::domain::email::delivery_address;
use tor_contracts::mail::SignupMailSource;
use tor_mail::domain::quota::identity::QuotaKeySigner;
use zeroize::Zeroizing;

pub(super) fn capture(email: &str, verified_ip: Option<IpAddr>) -> Option<SignupMailSource> {
    if std::env::var("TOR_MAIL_CAPTURE_REGISTRATION").as_deref() != Ok("1") {
        return None;
    }
    let key = std::env::var("TOR_MAIL_QUOTA_KEY").ok().map(Zeroizing::new);
    let source = key
        .as_ref()
        .and_then(|key| source_for(email, verified_ip, key.as_str()));
    if source.is_none() {
        // Keep password signup available; no evidence means no mail permit.
        eprintln!("TOR_MAIL_CAPTURE_UNAVAILABLE");
    }
    source
}

fn source_for(email: &str, verified_ip: Option<IpAddr>, key: &str) -> Option<SignupMailSource> {
    let address = delivery_address(email).ok()?;
    let signer = QuotaKeySigner::from_hex(key)?;
    Some(signer.keys(&address, verified_ip).ok()?.signup_source())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn capture_uses_canonical_idna_and_verified_ingress_not_forged_headers() {
        let key = "4f".repeat(32);
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.99"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.98"));
        let peer = "192.0.2.7:12345".parse().unwrap();
        let audit = super::super::access_audit::context(
            &headers,
            peer,
            tor_accounts::domain::access_audit::LoginEventKind::Registration,
        );
        let actual = source_for("Reader@пример.рф", audit.verified_ip, &key).unwrap();
        let expected = source_for(
            "reader@xn--e1afmkfd.xn--p1ai",
            Some("192.0.2.7".parse().unwrap()),
            &key,
        )
        .unwrap();
        assert!(actual == expected);
        assert!(source_for("reader@@example.com", audit.verified_ip, &key).is_none());
        assert!(source_for("reader@example.com", audit.verified_ip, "").is_none());
        assert!(source_for("reader@example.com", None, &key)
            .unwrap()
            .registration_ip_key
            .is_none());
    }
}
