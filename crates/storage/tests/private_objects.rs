use std::time::Duration;
use tor_contracts::paid_delivery::{
    ContentCodec, ContractError, DeliveryError, EncryptedObjectKind, EncryptedObjectRef, ObjectId,
    ObjectRef, PrivateObjectDeliveryPort,
};
use tor_storage::{
    domain::object_keys::{PrivateObjectKey, ENCRYPTED_CONTENT_TYPE},
    services::unconfigured::UnconfiguredPrivateDelivery,
};

fn uuid() -> ObjectId {
    ObjectId::parse("5e64be03-0b89-4cf7-b773-21e1f45344de").unwrap()
}

#[test]
fn immutable_keys_are_kind_scoped_and_canonical() {
    let chapter = PrivateObjectKey::for_object(EncryptedObjectKind::Chapter, &uuid());
    let asset = PrivateObjectKey::for_object(EncryptedObjectKind::ChapterAsset, &uuid());
    assert_eq!(
        chapter.as_str(),
        "private/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc"
    );
    assert_eq!(
        asset.as_str(),
        "private/chapter-assets/5e64be03-0b89-4cf7-b773-21e1f45344de.enc"
    );
    assert_eq!(PrivateObjectKey::parse(chapter.as_str()), Ok(chapter));
    assert_eq!(PrivateObjectKey::parse(asset.as_str()), Ok(asset));
    assert_eq!(ENCRYPTED_CONTENT_TYPE, "application/octet-stream");
}

#[test]
fn arbitrary_buckets_hosts_and_path_traversal_are_never_accepted() {
    for raw in [
        "/private/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc",
        "public/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc",
        "private/chapter-versions/../5e64be03-0b89-4cf7-b773-21e1f45344de.enc",
        "private/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc?versionId=x",
        "private/chapter-assets/5E64BE03-0B89-4CF7-B773-21E1F45344DE.enc",
        "https://bucket.example/private/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc",
        "private/chapter-versions/5e64be03-0b89-4cf7-b773-21e1f45344de.enc/other",
        "private/chapter-versions/%2e%2e/5e64be03-0b89-4cf7-b773-21e1f45344de.enc",
    ] {
        assert_eq!(
            PrivateObjectKey::parse(raw),
            Err(ContractError::InvalidObjectId),
            "{raw}"
        );
    }
}

#[tokio::test]
async fn unconfigured_storage_never_returns_a_signed_url_or_dek() {
    let object = ObjectRef::new(uuid(), EncryptedObjectKind::Chapter, [42; 32], 256).unwrap();
    let encrypted = EncryptedObjectRef {
        object,
        nonce: [7; 12],
        aad: b"tor.encrypted.chapter.v1\n1\n2\n".to_vec(),
        wrapped_key_id: "content-kek-v1".to_owned(),
        codec: ContentCodec::Gzip,
    };
    let adapter = UnconfiguredPrivateDelivery;
    for ttl in [
        Duration::from_secs(30),
        Duration::from_secs(90),
        Duration::from_secs(120),
    ] {
        let result = adapter.grant_verified_object(&encrypted, ttl).await;
        assert!(matches!(result, Err(DeliveryError::Unavailable)));
    }
}
