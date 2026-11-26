//! MAIL-01g2b: authenticate bounded relay bytes before parsing any JSON.
use super::{valid_id, PostboxEvent, MAX_EVENT_BYTES};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

const PREFIX: &[u8] = b"tor/postbox/relay/v1\n";
const MAX_SKEW_SECONDS: u64 = 300;

pub struct RelayVerifier {
    key: Zeroizing<[u8; 32]>,
    identity: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayError {
    Unauthorized,
    InvalidPayload,
    ForeignIdentity,
}

impl RelayVerifier {
    pub fn from_hex(key: &str, identity: &str) -> Option<Self> {
        if key.len() != 64 || !valid_id(identity) {
            return None;
        }
        let mut secret = Zeroizing::new([0_u8; 32]);
        hex::decode_to_slice(key, &mut secret[..]).ok()?;
        if secret.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self {
            key: secret,
            identity: identity.to_owned(),
        })
    }

    /// A timestamp limits replay of captured requests; the durable event-ID
    /// transaction provides the idempotency fence across legitimate retries.
    pub fn authenticate(
        &self,
        timestamp: &str,
        signature: &str,
        body: &[u8],
        now: u64,
    ) -> Result<Option<PostboxEvent>, RelayError> {
        if body.len() > MAX_EVENT_BYTES {
            return Err(RelayError::InvalidPayload);
        }
        let sent = timestamp
            .parse::<u64>()
            .map_err(|_| RelayError::Unauthorized)?;
        if sent.to_string() != timestamp
            || now.abs_diff(sent) > MAX_SKEW_SECONDS
            || signature.len() != 64
            || signature.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err(RelayError::Unauthorized);
        }
        let mut provided = [0_u8; 32];
        hex::decode_to_slice(signature, &mut provided).map_err(|_| RelayError::Unauthorized)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key[..]).expect("HMAC accepts a 256-bit key");
        mac.update(PREFIX);
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(body);
        mac.verify_slice(&provided)
            .map_err(|_| RelayError::Unauthorized)?;

        let event = PostboxEvent::parse(body).map_err(|_| RelayError::InvalidPayload)?;
        if event
            .as_ref()
            .is_some_and(|event| event.identity_id() != self.identity)
        {
            return Err(RelayError::ForeignIdentity);
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier() -> RelayVerifier {
        RelayVerifier::from_hex(&"a5".repeat(32), "identity-fixture").unwrap()
    }

    fn sign(body: &[u8], timestamp: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&[0xa5; 32]).unwrap();
        mac.update(PREFIX);
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn independent_python_hmac_vector_and_exact_body_binding() {
        let auth = verifier();
        let body = br#"{"eventType":"Delivery"}"#;
        let signature = "6c2eb55dcb191904155f2beac2f4629402dc8d7115fad6598a229d1d8076751f";
        assert!(auth
            .authenticate("1700000000", signature, body, 1700000300)
            .unwrap()
            .is_none());
        for (timestamp, input, now) in [
            ("1700000000", body.as_slice(), 1700000301),
            ("1700000000", body.as_slice(), 1699999699),
            ("01700000000", body.as_slice(), 1700000000),
            ("1700000001", body.as_slice(), 1700000000),
            ("1700000000", b"{}".as_slice(), 1700000000),
        ] {
            assert!(matches!(
                auth.authenticate(timestamp, signature, input, now),
                Err(RelayError::Unauthorized)
            ));
        }
        assert!(matches!(
            auth.authenticate("1700000000", &"00".repeat(32), body, 1700000000),
            Err(RelayError::Unauthorized)
        ));
        assert!(RelayVerifier::from_hex(&"00".repeat(32), "identity").is_none());
        assert!(RelayVerifier::from_hex(&"a5".repeat(32), "identity\n").is_none());
    }

    #[test]
    fn signature_does_not_authorize_other_identity_or_malformed_payload() {
        let auth = verifier();
        let timestamp = "1700000000";
        let event = serde_json::json!({
            "eventType":"Complaint",
            "eventId":"event-1",
            "mail":{"identityId":"foreign","messageId":"message-1"},
            "complaint":{
                "complaintFeedbackType":"abuse",
                "complainedRecipients":[{"emailAddress":"reader@example.invalid"}]
            }
        });
        let mut good = event.clone();
        good["mail"]["identityId"] = serde_json::json!("identity-fixture");
        let good = serde_json::to_vec(&good).unwrap();
        assert!(auth
            .authenticate(timestamp, &sign(&good, timestamp), &good, 1700000000)
            .unwrap()
            .is_some());
        let foreign = serde_json::to_vec(&event).unwrap();
        assert!(matches!(
            auth.authenticate(timestamp, &sign(&foreign, timestamp), &foreign, 1700000000),
            Err(RelayError::ForeignIdentity)
        ));
        let invalid = b"not json";
        assert!(matches!(
            auth.authenticate(timestamp, &sign(invalid, timestamp), invalid, 1700000000),
            Err(RelayError::InvalidPayload)
        ));
        assert!(matches!(
            auth.authenticate(timestamp, "", &vec![0; MAX_EVENT_BYTES + 1], 1700000000),
            Err(RelayError::InvalidPayload)
        ));
    }
}
