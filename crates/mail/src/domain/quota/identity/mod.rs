//! MAIL-01f2a: domain-separated, versioned quota identifiers.
//! Secret material is provisioned and recovered by the operator, never logged.
use super::QuotaKeys;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::IpAddr;
use zeroize::Zeroizing;

pub struct QuotaKeySigner {
    key: Zeroizing<[u8; 32]>,
}

impl QuotaKeySigner {
    /// Accept a separately provisioned 256-bit secret. No fallback, automatic
    /// regeneration, default key, Debug implementation or serialization.
    pub fn from_hex(value: &str) -> Option<Self> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut key = Zeroizing::new([0_u8; 32]);
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
        }
        if key.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self { key })
    }

    /// Call only with the output of accounts' validated delivery-address
    /// policy (ASCII local part and IDNA ASCII domain). This is a trust
    /// boundary, not a second mailbox validator. Never pass a browser-supplied
    /// forwarded IP: the caller must use the verified ingress address.
    pub fn keys(
        &self,
        transport_address: &str,
        verified_registration_ip: Option<IpAddr>,
    ) -> Result<QuotaKeys, &'static str> {
        if !transport_address.is_ascii()
            || transport_address.len() > 254
            || transport_address
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err("canonical transport address required");
        }
        let (local, domain) = transport_address
            .split_once('@')
            .ok_or("canonical transport address required")?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err("canonical transport address required");
        }
        // TOR already treats account email case as insensitive. Do not
        // remove dots, plus aliases or rewrite one provider into another.
        let canonical = Zeroizing::new(transport_address.to_ascii_lowercase());
        Ok(QuotaKeys {
            recipient: self.digest(b"tor/mail/quota/v1/recipient\0", canonical.as_bytes()),
            registration_ip: verified_registration_ip
                .map(|ip| match ip {
                    IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
                    _ => ip,
                })
                .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
                .map(|ip| self.ip_key(ip)),
            key_binding: self.digest(b"tor/mail/quota/v1/key-binding\0", b""),
            registration_intent: None,
            reset_intent: None,
        })
    }

    /// The worker resolves committed account evidence and the current
    /// delivery address. A later IP cannot replace the captured signup IP.
    pub fn keys_for_registration(
        &self,
        intent: tor_contracts::mail::MailIntent,
        current_transport_address: &str,
        evidence: tor_contracts::mail::RegistrationMailEvidence,
    ) -> Result<QuotaKeys, &'static str> {
        if intent.purpose != tor_contracts::mail::MailPurpose::RegistrationConfirmation
            || intent.recipient != evidence.user_id
        {
            return Err("registration mail evidence does not match intent");
        }
        let mut keys = self.keys(current_transport_address, None)?;
        if keys.recipient != evidence.source.recipient_key
            || keys.key_binding != evidence.source.key_binding
        {
            return Err("registration mail evidence is no longer valid");
        }
        keys.registration_ip = evidence.source.registration_ip_key;
        keys.registration_intent = Some(intent);
        Ok(keys)
    }

    /// Use the original trusted request IP, not the current worker/HTTP peer.
    /// The exact purpose, event ID, recipient, address and key must match.
    pub fn keys_for_password_reset(
        &self,
        intent: tor_contracts::mail::MailIntent,
        current_transport_address: &str,
        evidence: tor_contracts::mail::PasswordResetMailEvidence,
    ) -> Result<QuotaKeys, &'static str> {
        if intent.purpose != tor_contracts::mail::MailPurpose::PasswordReset
            || intent != evidence.intent
        {
            return Err("password reset mail evidence does not match intent");
        }
        let mut keys = self.keys(current_transport_address, None)?;
        if keys.recipient != evidence.source.recipient_key
            || keys.key_binding != evidence.source.key_binding
        {
            return Err("password reset mail evidence is no longer valid");
        }
        keys.registration_ip = evidence.source.request_ip_key;
        keys.reset_intent = Some(intent);
        Ok(keys)
    }

    fn ip_key(&self, ip: IpAddr) -> [u8; 32] {
        match ip {
            IpAddr::V4(ip) => self.digest(b"tor/mail/quota/v1/ip/v4\0", &ip.octets()),
            IpAddr::V6(ip) => {
                if let Some(mapped) = ip.to_ipv4_mapped() {
                    self.digest(b"tor/mail/quota/v1/ip/v4\0", &mapped.octets())
                } else {
                    self.digest(b"tor/mail/quota/v1/ip/v6\0", &ip.octets())
                }
            }
        }
    }

    fn digest(&self, namespace: &[u8], value: &[u8]) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key[..]).expect("HMAC accepts a 256-bit key");
        mac.update(namespace);
        mac.update(value);
        mac.finalize().into_bytes().into()
    }
}

#[cfg(test)]
mod tests;
