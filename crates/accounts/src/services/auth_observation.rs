//! HMAC pseudonyms for authentication-risk analysis, independent of RIL consent.
//! This key must be provisioned separately from cookies, password reset and RIL.
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::IpAddr;

#[derive(Clone)]
pub struct AuthObservationSigner {
    key: [u8; 32],
}

impl AuthObservationSigner {
    pub fn from_hex(value: &str) -> Option<Self> {
        if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut key = [0; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[2 * index..2 * index + 2], 16).ok()?;
        }
        if key.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self { key })
    }

    fn digest(&self, scope: &[u8], value: &[u8]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("32-byte key");
        mac.update(b"tor/auth/observations/v1/");
        mac.update(scope);
        mac.update(&[0]);
        mac.update(value);
        let mut digest = [0; 32];
        digest.copy_from_slice(&mac.finalize().into_bytes());
        digest
    }

    /// The caller passes only the same normalized email used for account lookup.
    pub fn target(&self, normalized_email: &str) -> [u8; 32] {
        self.digest(b"target", normalized_email.as_bytes())
    }

    /// Distinguish a single verified address from its /24 or /64 network.
    /// Neither digest can be reversed without the separately managed secret.
    pub fn source(&self, ip: IpAddr) -> ([u8; 32], [u8; 32]) {
        match ip {
            IpAddr::V4(value) => {
                let bytes = value.octets();
                let mut exact = [0u8; 5];
                exact[0] = 4;
                exact[1..].copy_from_slice(&bytes);
                let mut network = exact;
                network[4] = 0; // /24
                (self.digest(b"ip", &exact), self.digest(b"net", &network))
            }
            IpAddr::V6(value) => {
                // IPv4-mapped v6 addresses must not acquire distinct identities.
                if let Some(v4) = value.to_ipv4_mapped() {
                    return self.source(IpAddr::V4(v4));
                }
                let bytes = value.octets();
                let mut exact = [0u8; 17];
                exact[0] = 6;
                exact[1..].copy_from_slice(&bytes);
                let mut network = exact;
                network[9..].fill(0); // /64
                (self.digest(b"ip", &exact), self.digest(b"net", &network))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_key_spaces_and_normalized_identity_are_stable() {
        assert!(AuthObservationSigner::from_hex("short").is_none());
        assert!(AuthObservationSigner::from_hex(&"00".repeat(32)).is_none());
        let signer = AuthObservationSigner::from_hex(&"a1".repeat(32)).unwrap();
        assert_eq!(
            signer.target("a@example.invalid"),
            signer.target("a@example.invalid")
        );
        assert_ne!(
            signer.target("a@example.invalid"),
            signer.target("b@example.invalid")
        );
        assert_ne!(
            signer.target("1.2.3.4"),
            signer.source("1.2.3.4".parse().unwrap()).0
        );
        let other = AuthObservationSigner::from_hex(&"b2".repeat(32)).unwrap();
        assert_ne!(
            signer.target("a@example.invalid"),
            other.target("a@example.invalid")
        );
    }

    #[test]
    fn address_and_subnet_pseudonyms_are_distinct_and_bounded() {
        let key = AuthObservationSigner::from_hex(&"c3".repeat(32)).unwrap();
        let (ip_a, subnet_a) = key.source("198.51.100.1".parse().unwrap());
        let (ip_b, subnet_b) = key.source("198.51.100.254".parse().unwrap());
        assert_ne!(ip_a, ip_b);
        assert_eq!(subnet_a, subnet_b);
        assert_ne!(subnet_a, key.source("198.51.101.1".parse().unwrap()).1);
        let a = key.source("2001:db8:42:7::1".parse().unwrap());
        let b = key.source("2001:db8:42:7::2".parse().unwrap());
        assert_ne!(a.0, b.0);
        assert_eq!(a.1, b.1);
        assert_eq!(key.source("::ffff:198.51.100.1".parse().unwrap()).0, ip_a);
    }
}
