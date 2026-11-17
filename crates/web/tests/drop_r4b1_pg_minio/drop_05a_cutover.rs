//! DROP-05a: real G independent proof + B atomic PG activation.
//! Extracted from R4b1 to keep integration test modules below layout limits.
use super::*;
use tor_books::repositories::free_pack_activation::PackActivationClaim;
use tor_books::repositories::free_pack_rollback::PackRollback;
use tor_books::repositories::{free_pack_cutover::PackActivation, free_pack_jobs::PackJobClaim};
use tor_web::free_pack_worker::{
    activate_one_queued_verified_pack, activate_one_verified_pack, rollback_one_verified_pack,
};

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

    // The R4a commit enqueues a durable activation in the same PG transaction.
    // Test the queued B/G cutover against the independent manual path.
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT status FROM free_pack_activation_jobs WHERE pack_job_id={}",
                claim.job_id
            )
        ),
        "pending"
    );
    pg(
        url,
        &format!(
            "UPDATE free_pack_activation_jobs
             SET status='running',lease_epoch=1,attempts=1,
                 locked_until=clock_timestamp()+interval '2 minutes'
             WHERE pack_job_id={}",
            claim.job_id
        ),
    );
    let queued = PackActivationClaim {
        pack: claim.clone(),
        activation_epoch: 1,
        attempts: 1,
    };
    let mut stale = queued.clone();
    stale.activation_epoch += 1;
    assert!(matches!(
        activate_one_queued_verified_pack(catalog, proof, single, &stale).await,
        Err(AuthorError::Conflict)
    ));
    // A real S3 proof cannot authorize a cutover after this worker's lease
    // has expired, even if the immutable bytes are perfectly valid.
    pg(
        url,
        &format!(
            "UPDATE free_pack_activation_jobs
             SET locked_until=clock_timestamp()-interval '1 second'
             WHERE pack_job_id={}",
            claim.job_id
        ),
    );
    assert!(matches!(
        activate_one_queued_verified_pack(catalog, proof, single, &queued).await,
        Err(AuthorError::Conflict)
    ));
    pg(
        url,
        &format!(
            "UPDATE free_pack_activation_jobs
             SET locked_until=clock_timestamp()+interval '2 minutes'
             WHERE pack_job_id={}",
            claim.job_id
        ),
    );
    let (one, two) = tokio::join!(
        activate_one_queued_verified_pack(catalog, proof, single, &queued),
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
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT status FROM free_pack_activation_jobs WHERE pack_job_id={}",
                claim.job_id
            )
        ),
        "succeeded",
        "the lease and full-member active CAS must commit together"
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
    assert!(matches!(
        catalog
            .load_pack_rollback_job(claim.job_id, claim.epoch + 1, BUCKET)
            .await,
        Err(AuthorError::Conflict)
    ));
    assert!(matches!(
        catalog
            .load_pack_rollback_job(claim.job_id, claim.epoch, "tor-rollback-drill-other")
            .await,
        Err(AuthorError::Conflict)
    ));
    let exact_job = catalog
        .load_pack_rollback_job(claim.job_id, claim.epoch, BUCKET)
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        rollback_one_verified_pack(catalog, single, &exact_job),
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
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_pack_rollback_receipts
                 WHERE job_id={} AND pack_id={pack_id} AND build_epoch={}
                   AND operator_role=session_user",
                claim.job_id, claim.epoch
            ),
        ),
        "1",
        "concurrent replay preserves one committed rollback receipt"
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
    // A later, independent SINGLE generation is not evidence that the
    // previous operator rollback was just committed. Refuse false replay.
    let (chapter, version, _) = &editions[0];
    pg(
        url,
        &format!(
            "INSERT INTO free_edition_storage_generations
             (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes)
             SELECT chapter_id,version_id,3,'single',content_sha256,size_bytes
             FROM free_edition_storage_generations
             WHERE chapter_id={chapter} AND version_id={version} AND generation=2"
        ),
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT promote_free_edition_storage({chapter},{version},2,3)")
        ),
        "t"
    );
    assert!(matches!(
        rollback_one_verified_pack(catalog, single, claim).await,
        Err(AuthorError::Conflict)
    ));
    let second: i64 = pg(
        url,
        &format!(
            "SELECT generation FROM free_edition_active_storage
             WHERE chapter_id={} AND version_id={}",
            editions[1].0, editions[1].1
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(second, 2, "stale operation cannot move other members");
    println!("DROP05C_SINGLE_ROLLBACK_PASS: missing source denied, held recovery, monotone 2-member CAS, exact replay, stale later generation denied, no deletes");
}
