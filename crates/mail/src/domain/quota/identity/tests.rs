use super::*;

fn signer(byte: &str) -> QuotaKeySigner {
    QuotaKeySigner::from_hex(&byte.repeat(32)).unwrap()
}

#[test]
fn no_default_missing_or_malformed_secret() {
    for key in ["", "ab", &"00".repeat(32), &"xz".repeat(32)] {
        assert!(QuotaKeySigner::from_hex(key).is_none());
    }
    assert!(QuotaKeySigner::from_hex(&"4F".repeat(32)).is_some());
}

#[test]
fn stable_casefolding_and_separate_domains_without_alias_rewriting() {
    let first = signer("4f");
    let recovered = signer("4F");
    let other = signer("5a");
    let ip = Some("192.0.2.5".parse().unwrap());
    let a = first.keys("Reader+Books@EXAMPLE.COM", ip).unwrap();
    let b = recovered.keys("reader+books@example.com", ip).unwrap();
    assert_eq!(a.recipient(), b.recipient());
    assert_eq!(a.registration_ip(), b.registration_ip());
    assert_eq!(a.key_binding(), b.key_binding());
    assert_ne!(a.recipient(), a.registration_ip().unwrap());
    assert_ne!(a.recipient(), a.key_binding());
    assert_ne!(
        a.recipient(),
        other
            .keys("reader+books@example.com", ip)
            .unwrap()
            .recipient()
    );
    assert_ne!(
        a.recipient(),
        first.keys("reader@example.com", ip).unwrap().recipient()
    );
    assert_ne!(
        a.key_binding(),
        other
            .keys("reader+books@example.com", ip)
            .unwrap()
            .key_binding()
    );
}

#[test]
fn mapped_ipv4_and_ipv6_spellings_share_their_canonical_identity() {
    let signer = signer("4f");
    let make = |ip: &str| {
        signer
            .keys("reader@example.com", Some(ip.parse().unwrap()))
            .unwrap()
    };
    assert_eq!(
        make("192.0.2.5").registration_ip(),
        make("::ffff:192.0.2.5").registration_ip()
    );
    assert_eq!(
        make("2001:db8::5").registration_ip(),
        make("2001:0db8:0:0:0:0:0:5").registration_ip()
    );
    assert_ne!(
        make("192.0.2.5").registration_ip(),
        make("192.0.2.6").registration_ip()
    );
    for ip in ["127.0.0.1", "::1", "0.0.0.0", "::", "::ffff:127.0.0.1"] {
        assert!(make(ip).registration_ip().is_none());
    }
    assert!(signer
        .keys("reader@example.com", None)
        .unwrap()
        .registration_ip()
        .is_none());
}

#[test]
fn transport_domain_must_be_precanonicalized_by_accounts() {
    let signer = signer("4f");
    for address in [
        "",
        "reader",
        "reader@@example.com",
        "reader@",
        "@example.com",
        "reader@example.com\n",
        "reader@пример.рф",
    ] {
        assert!(signer.keys(address, None).is_err());
    }
    let a = signer.keys("Reader@XN--E1AFMKFD.XN--P1AI", None).unwrap();
    let b = signer.keys("reader@xn--e1afmkfd.xn--p1ai", None).unwrap();
    assert_eq!(a.recipient(), b.recipient());
}

#[test]
fn registration_evidence_preserves_source_and_rejects_mismatches() {
    use tor_contracts::{
        auth::UserId,
        mail::{MailIntent, MailPurpose, RegistrationMailEvidence},
        paid_delivery::PositiveId,
    };
    let signer = signer("4f");
    let user = UserId::new(42).unwrap();
    let intent = MailIntent {
        purpose: MailPurpose::RegistrationConfirmation,
        event_id: PositiveId::new(99).unwrap(),
        recipient: user,
    };
    let captured = signer
        .keys("reader@example.com", Some("192.0.2.7".parse().unwrap()))
        .unwrap();
    let evidence = RegistrationMailEvidence {
        user_id: user,
        source: captured.signup_source(),
    };
    let keys = signer
        .keys_for_registration(intent, "Reader@EXAMPLE.COM", evidence)
        .unwrap();
    assert_eq!(keys.registration_ip(), captured.registration_ip());
    assert!(keys.matches_registration(intent));
    let mut different = intent;
    different.event_id = PositiveId::new(100).unwrap();
    assert!(!keys.matches_registration(different));
    assert!(signer
        .keys_for_registration(intent, "other@example.com", evidence)
        .is_err());
    let wrong_user = RegistrationMailEvidence {
        user_id: UserId::new(43).unwrap(),
        ..evidence
    };
    assert!(signer
        .keys_for_registration(intent, "reader@example.com", wrong_user)
        .is_err());
    let other = QuotaKeySigner::from_hex(&"5a".repeat(32)).unwrap();
    assert!(other
        .keys_for_registration(intent, "reader@example.com", evidence)
        .is_err());
    let mut not_registration = intent;
    not_registration.purpose = MailPurpose::SupportReply;
    assert!(signer
        .keys_for_registration(not_registration, "reader@example.com", evidence)
        .is_err());
}
