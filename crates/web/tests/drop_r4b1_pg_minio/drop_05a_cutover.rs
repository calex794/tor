//! DROP-05a: real G independent proof + B atomic PG activation.
//! Extracted from R4b1 to keep integration test modules below layout limits.
use super::*;
use tor_books::repositories::{free_pack_cutover::PackActivation, free_pack_jobs::PackJobClaim};
use tor_web::free_pack_worker::activate_one_verified_pack;

pub(super) async fn verify(
    url: &str,
    client: &Client,
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    single: &S3FreeChapterStore,
    claim: &PackJobClaim,
    book: i64,
    pack_id: i64,
    editions: &[(i64, i64, String)],
) {
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
}
