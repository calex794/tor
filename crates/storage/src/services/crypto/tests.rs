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
    assert_eq!(hex::encode(sha256_bytes(&ciphertext)), field("sha256_ciphertext"));
    let clear = decrypt_raw(&dek, &nonce, &aad, &ciphertext).unwrap();
    assert_eq!(clear, gzip);
    assert_eq!(decode_gzip(&clear), field("plaintext"));

    let wrapped = encrypt_raw(&kek, &wrap_nonce, &wrap_aad(&object_id()), &dek).unwrap();
    assert_eq!(hex::encode(&wrapped), field("wrapped_dek_hex"));
    let unwrapped = decrypt_raw(&kek, &wrap_nonce, &wrap_aad(&object_id()), &wrapped).unwrap();
    assert_eq!(unwrapped, dek);
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
    assert_eq!(
        unwrap_scoped_dek(&sealed, &ContentKek::from_bytes([8; 32])).unwrap_err(),
        CryptoError::AuthenticationFailed
    );
    let original = sealed.wrapped_dek.clone();
    sealed.wrapped_dek[0] ^= 1;
    assert_eq!(
        unwrap_scoped_dek(&sealed, &kek).unwrap_err(),
        CryptoError::AuthenticationFailed
    );
    sealed.wrapped_dek = original;
    sealed.object_id = ObjectId::parse("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    assert_eq!(
        unwrap_scoped_dek(&sealed, &kek).unwrap_err(),
        CryptoError::AuthenticationFailed
    );
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
    let mut zipped = gzip_markdown("data".as_bytes()).unwrap();
    assert_eq!(&zipped[4..8], &[0_u8; 4], "deterministic gzip mtime");
    zipped[4] = 1;
    assert_ne!(&zipped[4..8], &[0_u8; 4]);
}
