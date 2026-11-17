//! DROP-07 / D-05: a purchased historical Layer book remains a protected
//! physical FREE PACK and sidecar after ordinary owner deletion.
//! No deletion/mutation or cleanup is performed by this acceptance probe.
use super::{BUCKET, pg};
use aws_sdk_s3::Client;
use sha2::{Digest, Sha256};
use tor_books::repositories::{
    free_expected_inventory::{InventoryNamespace, PgFreeExpectedInventory},
    free_observed_reconcile::{FreeReferenceState, PgFreeObservedClassifier},
};
use tor_contracts::drop_pack::PackPlacementReceipt;
use tor_books::repositories::PgCatalog;

pub(super) async fn verify(
    url: &str,
    client: &Client,
    book: i64,
    receipt: &PackPlacementReceipt,
) {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    assert_eq!(
        pg(url, &format!("SELECT lifecycle_status FROM books WHERE id={book}")),
        "ordinary_deleted",
        "verify physical archive only after confirmed deletion"
    );
    let pool = PgCatalog::connect(url).await.unwrap().pool().clone();
    let keys = vec![
        receipt.target.pack_key.clone(),
        receipt.target.manifest_key.clone(),
    ];
    let classification = PgFreeObservedClassifier::new(pool.clone())
        .classify_page(BUCKET, &keys)
        .await
        .unwrap();
    assert_eq!(classification.len(), 2);
    for key in &keys {
        assert!(
            classification.iter().any(|entry|
                entry.object_key == *key && entry.state == FreeReferenceState::Registered
            ),
            "archived physical PACK/manifest must never be quarantined as unreferenced: {key}"
        );
    }
    let index = PgFreeExpectedInventory::new(pool.clone());
    let mut cursor: Option<String> = None;
    let mut seen_pack = false;
    let mut seen_manifest = false;
    for _ in 0..16 {
        let page = index
            .expected_page_in_namespace(
                BUCKET, InventoryNamespace::Pack, cursor.as_deref(), 128,
            )
            .await
            .unwrap();
        for item in &page.expected {
            if item.object_key == receipt.target.pack_key {
                assert_eq!(item.sha256, receipt.target.pack_sha256);
                assert_eq!(item.size_bytes, receipt.target.pack_bytes);
                seen_pack = true;
            }
            if item.object_key == receipt.target.manifest_key {
                assert_eq!(item.sha256, receipt.target.manifest_sha256);
                assert_eq!(item.size_bytes, receipt.target.manifest_bytes);
                seen_manifest = true;
            }
        }
        cursor = page.next_after_key;
        if cursor.is_none() || (seen_pack && seen_manifest) {
            break;
        }
    }
    assert!(seen_pack && seen_manifest, "both immutable PACK and sidecar remain expected");

    // Inventory alone is not byte evidence. Probe private MinIO after
    // ordinary deletion; do not infer retention from SQL receipts only.
    let pack = client
        .head_object()
        .bucket(BUCKET)
        .key(&receipt.target.pack_key)
        .send()
        .await
        .unwrap();
    assert_eq!(pack.content_length(), Some(receipt.target.pack_bytes as i64));
    let manifest = client
        .get_object()
        .bucket(BUCKET)
        .key(&receipt.target.manifest_key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(manifest.len() as u64, receipt.target.manifest_bytes);
    let actual: [u8; 32] = Sha256::digest(&manifest).into();
    assert_eq!(actual, receipt.target.manifest_sha256, "archived sidecar is byte-perfect");
    pool.close().await;
}
