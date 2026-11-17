//! DROP-05a: real G independent proof + B atomic PG activation.
//! Extracted from R4b1 to keep integration test modules below layout limits.
use super::*;
use tor_books::repositories::free_pack_rollback::PackRollback;
use tor_books::repositories::{free_pack_cutover::PackActivation, free_pack_jobs::PackJobClaim};
use tor_web::free_pack_worker::{activate_one_verified_pack, rollback_one_verified_pack};

pub(super) async fn verify(
    url: &str,
    client: &Client,
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    single: &S3FreeChapterStore,
    claim: &PackJobClaim,
    subject: (i64, i64, &[(i64, i64, String)]),
) {
    let (book, pack_id, editions) = subject;
    // DROP-05a: no startup activation. Explicit trusted G proof must precede
    // the all-or-none B promotion, and legal hold always wins.
    pg(
        url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert!(matches!(
        activate_one_verified_pack(catalog, proof, single, claim).await,
        Err(AuthorError::Conflict)
    ));
    let no_active: i64 = pg(
        url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage
             WHERE chapter_id IN ({},{})",
            editions[0].0, editions[1].0
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        no_active, 0,
        "moderation hold cannot partially activate PACK"
    );
    pg(
        url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );

    // A missing original SINGLE must block cutover even though the PACK was
    // already proved. Restore exact immutable source before trying again.
    let first_source =
        FreeChapterObject::for_markdown(editions[0].0, editions[0].1, &editions[0].2).unwrap();
    client
        .delete_object()
        .bucket(BUCKET)
        .key(first_source.key())
        .send()
        .await
        .unwrap();
    assert!(matches!(
        activate_one_verified_pack(catalog, proof, single, claim).await,
        Err(AuthorError::Unavailable)
    ));
    let still_zero: i64 = pg(
        url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage
             WHERE chapter_id IN ({},{})",
            editions[0].0, editions[1].0
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(still_zero, 0);
    upload_and_verify(single, &first_source, &editions[0].2)
        .await
        .unwrap();

    let (one, two) = tokio::join!(
        activate_one_verified_pack(catalog, proof, single, claim),
        activate_one_verified_pack(catalog, proof, single, claim),
    );
    assert!(matches!(
        (one.unwrap(), two.unwrap()),
        (PackActivation::Activated, PackActivation::AlreadyActive)
            | (PackActivation::AlreadyActive, PackActivation::Activated)
    ));
    assert_eq!(
        activate_one_verified_pack(catalog, proof, single, claim)
            .await
            .unwrap(),
        PackActivation::AlreadyActive,
        "lost COMMIT ACK cannot create another activation"
    );
    let active_pack: i64 = pg(
        url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage a
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation
             WHERE g.pack_id={pack_id} AND g.placement_kind='pack'"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        active_pack, 2,
        "both editions activate in one PG transaction"
    );
    for (chapter, version, _) in editions {
        let original_single: i64 = pg(
            url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={chapter} AND version_id={version}"
            ),
        )
        .parse()
        .unwrap();
        assert_eq!(original_single, 1, "no original SINGLE was removed");
    }
    println!("DROP05A_G_REPROOF_CAS_PASS: real PG18/MinIO, 2 frames, hold, race, replay, retained SINGLE");

    // DROP-05c: recovery is explicit, never an automatic reader fallback.
    // The original SINGLE must still be independently readable; no active
    // pointer may move if even one original object is missing.
    let first =
        FreeChapterObject::for_markdown(editions[0].0, editions[0].1, &editions[0].2).unwrap();
    client
        .delete_object()
        .bucket(BUCKET)
        .key(first.key())
        .send()
        .await
        .unwrap();
    assert!(matches!(
        rollback_one_verified_pack(catalog, single, claim).await,
        Err(AuthorError::Unavailable)
    ));
    let before: i64 = pg(
        url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage a
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation
             WHERE g.pack_id={pack_id} AND g.placement_kind='pack'"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(before, 2, "no partial rollback after missing source");
    upload_and_verify(single, &first, &editions[0].2)
        .await
        .unwrap();

    // Restore even while the book is held: this action changes physical
    // placement, not reader entitlements. The hold remains authoritative.
    pg(
        url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let (one, two) = tokio::join!(
        rollback_one_verified_pack(catalog, single, claim),
        rollback_one_verified_pack(catalog, single, claim),
    );
    assert!(matches!(
        (one.unwrap(), two.unwrap()),
        (PackRollback::RolledBack, PackRollback::AlreadyRolledBack)
            | (PackRollback::AlreadyRolledBack, PackRollback::RolledBack)
    ));
    assert_eq!(
        rollback_one_verified_pack(catalog, single, claim)
            .await
            .unwrap(),
        PackRollback::AlreadyRolledBack,
        "lost rollback commit ACK must not create extra generations"
    );
    let new_single: i64 = pg(
        url,
        &format!(
            "SELECT count(*) FROM free_edition_active_storage a
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation
             JOIN chapters c ON c.id=a.chapter_id
             WHERE c.book_id={book} AND g.placement_kind='single'
               AND g.generation>1 AND g.pack_id IS NULL"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(new_single, 2, "both members need higher SINGLE generations");
    let verified_pack = client
        .head_object()
        .bucket(BUCKET)
        .key(&claim.receipt.target.pack_key)
        .send()
        .await;
    assert!(verified_pack.is_ok(), "PACK retained until DROP-07 GC");
    pg(
        url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    println!("DROP05C_SINGLE_ROLLBACK_PASS: missing source denied, held recovery, monotone 2-member CAS, replay, no deletes");
}
