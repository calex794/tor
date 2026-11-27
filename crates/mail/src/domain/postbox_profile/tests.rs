use super::*;

fn key(version: i32, byte: u8) -> ProfileMasterKey {
    ProfileMasterKey::from_base64(version, &STANDARD.encode([byte; 32])).unwrap()
}

fn identity() -> ProfileIdentity {
    ProfileIdentity::new("identity-1".to_owned(), "support@pandaolate.ru".to_owned()).unwrap()
}

#[test]
fn encrypted_profile_survives_reopen_but_cannot_be_substituted() {
    let master = key(1, 13);
    let secrets = ProfileSecrets::new("fixture-access".into(), "fixture-secret".into()).unwrap();
    let mut sealed = master.seal(identity(), 2, &secrets).unwrap();
    let another = master.seal(identity(), 2, &secrets).unwrap();
    assert_ne!(sealed.nonce, another.nonce);
    assert_ne!(sealed.intent_digest(), another.intent_digest());
    assert!(!String::from_utf8_lossy(&sealed.ciphertext).contains("fixture-secret"));
    let reopened = key(1, 13).open(&sealed).unwrap();
    assert_eq!(reopened.access(), "fixture-access");
    assert_eq!(reopened.secret(), "fixture-secret");
    assert!(matches!(
        key(1, 14).open(&sealed),
        Err(SealError::KeyUnavailable)
    ));
    assert!(matches!(
        key(2, 13).open(&sealed),
        Err(SealError::KeyUnavailable)
    ));

    sealed.revision += 1;
    assert!(matches!(
        master.open(&sealed),
        Err(SealError::KeyUnavailable)
    ));
    sealed.revision -= 1;
    sealed.identity.sender = "other@pandaolate.ru".into();
    assert!(matches!(
        master.open(&sealed),
        Err(SealError::KeyUnavailable)
    ));
    sealed.identity = identity();
    sealed.ciphertext[0] ^= 1;
    assert!(matches!(
        master.open(&sealed),
        Err(SealError::KeyUnavailable)
    ));
}

#[test]
fn rejects_malformed_credentials_keys_and_sender() {
    for sender in ["sender", "a@@example.com", "a@127.0.0.1", "a@example.com\n"] {
        assert!(ProfileIdentity::new("id".into(), sender.into()).is_err());
    }
    assert!(ProfileSecrets::new("id".into(), "bad\nsecret".into()).is_err());
    assert!(ProfileMasterKey::from_base64(1, &STANDARD.encode([0; 32])).is_err());
    assert!(ProfileMasterKey::from_base64(0, &STANDARD.encode([7; 32])).is_err());
    assert!(ProfileMasterKey::from_base64(1, "bad").is_err());
}
