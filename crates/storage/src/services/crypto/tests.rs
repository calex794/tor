use super::*;
use flate2::read::GzDecoder;
use std::io::Read;
use tor_contracts::paid_delivery::PositiveId;

fn version() -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(123).unwrap(),
        version_id: PositiveId::new(456).unwrap(),
    }
}

fn object_id() -> ObjectId {
    ObjectId::parse("cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61").unwrap()
}

fn fixed_hex<const N: usize>(value: &str) -> [u8; N] {
    hex::decode(value).unwrap().try_into().unwrap()
}

fn decode_gzip(data: &[u8]) -> String {
    let mut decoder = GzDecoder::new(data);
    let mut output = String::new();
    decoder.read_to_string(&mut output).unwrap();
    output
}

#[test]
fn independent_golden_vector_matches_aesgcm_and_wrap_aad() {
    // Produced independently with Python cryptography.AESGCM + gzip, mtime=0.
    // This fixture is also reusable in browser WebCrypto regression tests.
    let vector: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/chapter_v1.json")).unwrap();
    let field = |key| vector[key].as_str().unwrap();
    let dek = fixed_hex::<32>(field("dek_hex"));
    let nonce = fixed_hex::<12>(field("nonce_hex"));
    let kek = fixed_hex::<32>(field("kek_hex"));
    let wrap_nonce = fixed_hex::<12>(field("wrap_nonce_hex"));
    let gzip = hex::decode(field("gzip_hex")).unwrap();
    let aad = chapter_aad(
        version(),
        &object_id(),
        &fixed_hex::<32>(field("plaintext_sha256")),
    );
    assert_eq!(hex::encode(&aad), field("aad_hex"));
    assert_eq!(hex::encode(wrap_aad(&object_id())), field("wrap_aad_hex"));

    let ciphertext = encrypt_raw(&dek, &nonce, &aad, &gzip).unwrap();
    assert_eq!(hex::encode(&ciphertext), field("ciphertext_hex"));
    assert_eq!(
        hex::encode(sha256_bytes(&ciphertext)),
        field("sha256_ciphertext")
    );
    let clear = decrypt_raw(&dek, &nonce, &aad, &ciphertext).unwrap();
    assert_eq!(clear, gzip);
    assert_eq!(decode_gzip(&clear), field("plaintext"));

    let wrapped = encrypt_raw(&kek, &wrap_nonce, &wrap_aad(&object_id()), &dek).unwrap();
    assert_eq!(hex::encode(&wrapped), field("wrapped_dek_hex"));
    let unwrapped = decrypt_raw(&kek, &wrap_nonce, &wrap_aad(&object_id()), &wrapped).unwrap();
    assert_eq!(unwrapped.as_slice(), dek.as_slice());
}

#[test]
fn random_envelopes_keep_pinned_version_and_plaintext_out_of_storage() {
    let kek = ContentKek::from_bytes([0x44; 32]);
    let source = "Первая глава\r\n\r\nSECRET-PAID-CONTENT \u{1f600}";
    let normalized = "Первая глава\n\nSECRET-PAID-CONTENT \u{1f600}";
    let first = seal_chapter(version(), object_id(), source, &kek).unwrap();
    let second = seal_chapter(version(), object_id(), source, &kek).unwrap();
    assert_ne!(first.object_nonce, second.object_nonce);
    assert_ne!(first.ciphertext, second.ciphertext);
    assert_ne!(first.wrap_nonce, second.wrap_nonce);
    assert_eq!(first.kek_id, "content-kek-v1");
    assert!(verify_ciphertext(&first, &first.ciphertext));
    assert_eq!(first.plaintext_sha256, sha256_bytes(normalized.as_bytes()));
    assert_eq!(
        first.aad,
        chapter_aad(version(), &object_id(), &first.plaintext_sha256)
    );
    assert!(!first
        .ciphertext
        .windows("SECRET-PAID-CONTENT".len())
        .any(|w| w == b"SECRET-PAID-CONTENT"));
    let dek = unwrap_scoped_dek(&first, &kek).unwrap();
    let gzip = decrypt_raw(&dek, &first.object_nonce, &first.aad, &first.ciphertext).unwrap();
    assert_eq!(decode_gzip(&gzip), normalized);
    let log = format!("{first:?} {kek:?}");
    assert!(!log.contains(normalized));
    assert!(!log.contains("wrapped_dek"));
    assert!(log.contains("[REDACTED]"));
}

#[test]
fn tampered_aes_gcm_tag_nonce_aad_and_dek_never_decrypt() {
    let kek = ContentKek::from_bytes([7; 32]);
    let sealed = seal_chapter(version(), object_id(), "Платный текст", &kek).unwrap();
    let dek = unwrap_scoped_dek(&sealed, &kek).unwrap();
    let original = &sealed.ciphertext;
    let mut changed = original.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    assert!(!verify_ciphertext(&sealed, &changed));
    assert_eq!(
        decrypt_raw(&dek, &sealed.object_nonce, &sealed.aad, &changed),
        Err(CryptoError::AuthenticationFailed)
    );
    let mut other_nonce = sealed.object_nonce;
    other_nonce[0] ^= 1;
    assert_eq!(
        decrypt_raw(&dek, &other_nonce, &sealed.aad, original),
        Err(CryptoError::AuthenticationFailed)
    );
    let mut wrong_aad = sealed.aad.clone();
    wrong_aad[0] ^= 1;
    assert_eq!(
        decrypt_raw(&dek, &sealed.object_nonce, &wrong_aad, original),
        Err(CryptoError::AuthenticationFailed)
    );
    assert_eq!(
        decrypt_raw(&[9; 32], &sealed.object_nonce, &sealed.aad, original),
        Err(CryptoError::AuthenticationFailed)
    );
}

#[test]
fn wrong_kek_tampered_wrap_or_wrong_object_cannot_recover_key() {
    let kek = ContentKek::from_bytes([7; 32]);
    let mut sealed = seal_chapter(version(), object_id(), "Секрет", &kek).unwrap();
    assert!(unwrap_scoped_dek(&sealed, &kek).is_ok());
    assert!(matches!(
        unwrap_scoped_dek(&sealed, &ContentKek::from_bytes([8; 32])),
        Err(CryptoError::AuthenticationFailed)
    ));
    let original = sealed.wrapped_dek.clone();
    sealed.wrapped_dek[0] ^= 1;
    assert!(matches!(
        unwrap_scoped_dek(&sealed, &kek),
        Err(CryptoError::AuthenticationFailed)
    ));
    sealed.wrapped_dek = original;
    sealed.object_id = ObjectId::parse("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    assert!(matches!(
        unwrap_scoped_dek(&sealed, &kek),
        Err(CryptoError::AuthenticationFailed)
    ));
}

#[test]
fn invalid_size_empty_and_missing_material_fail_closed() {
    let kek = ContentKek::from_bytes([7; 32]);
    assert!(matches!(
        seal_chapter(version(), object_id(), "", &kek),
        Err(CryptoError::InvalidMaterial)
    ));
    assert!(matches!(
        seal_chapter(version(), object_id(), &"Ж".repeat(100_001), &kek),
        Err(CryptoError::InvalidMaterial)
    ));
    assert!(matches!(
        seal_chapter(version(), object_id(), &"a".repeat(50_001), &kek),
        Err(CryptoError::InvalidMaterial)
    ));
    let mut zipped = gzip_markdown("data".as_bytes()).unwrap();
    assert_eq!(&zipped[4..8], &[0_u8; 4], "deterministic gzip mtime");
    zipped[4] = 1;
    assert_ne!(&zipped[4..8], &[0_u8; 4]);
}

#[test]
fn s334_rewrap_preserves_paid_v1_ciphertext_but_changes_only_key_envelope() {
    let old = ContentKek::from_bytes([0x11; 32]);
    let next = ContentKek::from_bytes([0x22; 32]);
    let stored = seal_chapter(version(), object_id(), "Старая купленная V1 😀", &old).unwrap();
    let unchanged_ciphertext = sha256_bytes(&stored.ciphertext);
    let old_dek = unwrap_scoped_dek(&stored, &old).unwrap();

    let first = rewrap_registered_v1_to_v2(
        &stored.object_id,
        &stored.wrapped_dek,
        &stored.wrap_nonce,
        &old,
        &next,
    )
    .unwrap();
    let second = rewrap_registered_v1_to_v2(
        &stored.object_id,
        &stored.wrapped_dek,
        &stored.wrap_nonce,
        &old,
        &next,
    )
    .unwrap();
    assert_eq!(first.kek_id, "content-kek-v2");
    assert_ne!(first.wrap_nonce, stored.wrap_nonce);
    assert_ne!(first.wrap_nonce, second.wrap_nonce);
    assert_ne!(first.wrapped_dek, stored.wrapped_dek);
    assert_eq!(first.wrapped_dek.len(), 48);
    assert_eq!(sha256_bytes(&stored.ciphertext), unchanged_ciphertext);
    let new_dek = decrypt_raw(
        &next.0,
        &first.wrap_nonce,
        &wrap_aad(&stored.object_id),
        &first.wrapped_dek,
    )
    .unwrap();
    assert_eq!(new_dek.as_slice(), &old_dek[..]);
    assert_eq!(
        decrypt_raw(
            &old.0,
            &first.wrap_nonce,
            &wrap_aad(&stored.object_id),
            &first.wrapped_dek
        ),
        Err(CryptoError::AuthenticationFailed),
    );
    assert!(!format!("{first:?}").contains(&hex::encode(&first.wrapped_dek)));
    assert!(format!("{first:?}").contains("[REDACTED]"));
}

#[test]
fn s334_rewrap_rejects_wrong_old_key_tampering_wrong_object_and_fake_rotation() {
    let old = ContentKek::from_bytes([0x11; 32]);
    let next = ContentKek::from_bytes([0x22; 32]);
    let wrong = ContentKek::from_bytes([0x33; 32]);
    let stored = seal_chapter(version(), object_id(), "Pinned V1", &old).unwrap();
    assert!(matches!(
        rewrap_registered_v1_to_v2(
            &stored.object_id,
            &stored.wrapped_dek,
            &stored.wrap_nonce,
            &wrong,
            &next,
        ),
        Err(CryptoError::AuthenticationFailed)
    ));
    let mut tampered = stored.wrapped_dek.clone();
    tampered[0] ^= 1;
    assert!(matches!(
        rewrap_registered_v1_to_v2(
            &stored.object_id,
            &tampered,
            &stored.wrap_nonce,
            &old,
            &next,
        ),
        Err(CryptoError::AuthenticationFailed)
    ));
    let swapped = ObjectId::parse("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    assert!(matches!(
        rewrap_registered_v1_to_v2(
            &swapped,
            &stored.wrapped_dek,
            &stored.wrap_nonce,
            &old,
            &next,
        ),
        Err(CryptoError::AuthenticationFailed)
    ));
    assert!(matches!(
        rewrap_registered_v1_to_v2(
            &stored.object_id,
            &stored.wrapped_dek,
            &stored.wrap_nonce,
            &old,
            &old,
        ),
        Err(CryptoError::InvalidMaterial)
    ));
    assert!(matches!(
        rewrap_registered_v1_to_v2(
            &stored.object_id,
            &stored.wrapped_dek[..47],
            &stored.wrap_nonce,
            &old,
            &next,
        ),
        Err(CryptoError::UnavailableKey)
    ));
}

#[test]
fn s334_read_keyring_fails_closed_without_matching_v2_or_with_fake_id() {
    let old = Arc::new(ContentKek::from_bytes([0x72; 32]));
    let next = Arc::new(ContentKek::from_bytes([0x73; 32]));
    let sealed = seal_chapter(version(), object_id(), "Purchased V1", &old).unwrap();
    let prior = unwrap_registered_dek(
        &sealed.object_id,
        &sealed.wrapped_dek,
        &sealed.wrap_nonce,
        "content-kek-v1",
        &old,
    )
    .unwrap();
    let updated = rewrap_registered_v1_to_v2(
        &sealed.object_id,
        &sealed.wrapped_dek,
        &sealed.wrap_nonce,
        &old,
        &next,
    )
    .unwrap();

    let single = ContentKeyring::v1_only(old.clone());
    assert!(matches!(
        unwrap_registered_dek_from_keyring(
            &sealed.object_id,
            &updated.wrapped_dek,
            &updated.wrap_nonce,
            "content-kek-v2",
            &single,
        ),
        Err(CryptoError::UnavailableKey)
    ));
    assert!(matches!(
        ContentKeyring::with_v2(old.clone(), old.clone()),
        Err(CryptoError::InvalidMaterial)
    ));
    let both = ContentKeyring::with_v2(old, next).unwrap();
    let v1 = unwrap_registered_dek_from_keyring(
        &sealed.object_id,
        &sealed.wrapped_dek,
        &sealed.wrap_nonce,
        "content-kek-v1",
        &both,
    )
    .unwrap();
    let v2 = unwrap_registered_dek_from_keyring(
        &sealed.object_id,
        &updated.wrapped_dek,
        &updated.wrap_nonce,
        updated.kek_id,
        &both,
    )
    .unwrap();
    assert_eq!(&v1[..], &prior[..]);
    assert_eq!(&v2[..], &prior[..]);
    assert!(matches!(
        unwrap_registered_dek_from_keyring(
            &sealed.object_id,
            &updated.wrapped_dek,
            &updated.wrap_nonce,
            "content-kek-v99",
            &both,
        ),
        Err(CryptoError::UnavailableKey)
    ));
    assert!(matches!(
        unwrap_registered_dek_from_keyring(
            &sealed.object_id,
            &updated.wrapped_dek,
            &updated.wrap_nonce,
            "content-kek-v1",
            &both,
        ),
        Err(CryptoError::AuthenticationFailed)
    ));
}

#[test]
fn s334_new_paid_revision_uses_opted_in_v2_without_breaking_purchased_v1() {
    let old_key = Arc::new(ContentKek::from_bytes([0x91; 32]));
    let next_key = Arc::new(ContentKek::from_bytes([0x92; 32]));
    let legacy = ContentKeyring::with_v2(old_key.clone(), next_key.clone()).unwrap();
    assert_eq!(legacy.active_id(), "content-kek-v1");
    assert!(matches!(
        ContentKeyring::v1_only(old_key.clone()).with_active_id("content-kek-v2"),
        Err(CryptoError::UnavailableKey)
    ));
    assert!(matches!(
        ContentKeyring::with_v2(old_key.clone(), next_key.clone())
            .unwrap()
            .with_active_id(""),
        Err(CryptoError::UnavailableKey)
    ));
    assert!(matches!(
        ContentKeyring::with_v2(old_key.clone(), next_key.clone())
            .unwrap()
            .with_active_id("content-kek-v3"),
        Err(CryptoError::UnavailableKey)
    ));

    let purchased_v1 = seal_chapter_with_keyring(
        version(),
        object_id(),
        "Купленная V1 остаётся доступной",
        &legacy,
    )
    .unwrap();
    assert_eq!(purchased_v1.kek_id, "content-kek-v1");

    let active = legacy.with_active_id("content-kek-v2").unwrap();
    let next_object = ObjectId::parse("cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf62").unwrap();
    let new_v2 = seal_chapter_with_keyring(
        ChapterVersionRef {
            chapter_id: version().chapter_id,
            version_id: PositiveId::new(457).unwrap(),
        },
        next_object,
        "Новая платная V2: 文本",
        &active,
    )
    .unwrap();
    assert_eq!(new_v2.kek_id, "content-kek-v2");
    assert_ne!(purchased_v1.aad, new_v2.aad);
    assert_ne!(purchased_v1.ciphertext_sha256, new_v2.ciphertext_sha256);
    assert!(verify_ciphertext(&purchased_v1, &purchased_v1.ciphertext));
    assert!(verify_ciphertext(&new_v2, &new_v2.ciphertext));

    for (sealed, expected) in [
        (&purchased_v1, "Купленная V1 остаётся доступной"),
        (&new_v2, "Новая платная V2: 文本"),
    ] {
        let dek = unwrap_registered_dek_from_keyring(
            &sealed.object_id,
            &sealed.wrapped_dek,
            &sealed.wrap_nonce,
            sealed.kek_id,
            &active,
        )
        .unwrap();
        let compressed =
            decrypt_raw(&dek, &sealed.object_nonce, &sealed.aad, &sealed.ciphertext).unwrap();
        assert_eq!(decode_gzip(&compressed), expected);
    }
    assert!(matches!(
        unwrap_registered_dek_from_keyring(
            &new_v2.object_id,
            &new_v2.wrapped_dek,
            &new_v2.wrap_nonce,
            "content-kek-v1",
            &active,
        ),
        Err(CryptoError::AuthenticationFailed)
    ));
    assert!(matches!(
        unwrap_registered_dek_from_keyring(
            &new_v2.object_id,
            &new_v2.wrapped_dek,
            &new_v2.wrap_nonce,
            new_v2.kek_id,
            &ContentKeyring::v1_only(old_key),
        ),
        Err(CryptoError::UnavailableKey)
    ));
    assert!(!format!("{new_v2:?} {next_key:?}").contains("Новая платная"));
}
