//! G-09 bounded GET audit: durable cursor, pseudonymous evidence, and fail-closed retries.
use super::*;
use std::sync::atomic::AtomicUsize;
use tor_storage::{
    repositories::reconciliation_body::PgBodyAudit,
    services::reconciliation::{PrivateRegisteredBodyProbe, RegisteredBodyResult},
};

const BODY_BUCKET: &str = "tor-private-body-sha-ci";

#[derive(Default)]
struct FakeBody {
    fail_remaining: AtomicUsize,
    probes: AtomicUsize,
}

#[async_trait]
impl PrivateRegisteredBodyProbe for FakeBody {
    fn private_bucket(&self) -> &str {
        BODY_BUCKET
    }

    async fn probe_registered_body(
        &self,
        object: &tor_contracts::paid_delivery::ObjectRef,
    ) -> Result<RegisteredBodyResult, StorageError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(StorageError::Unavailable);
        }
        let id = u32::from_str_radix(
            object.object_id.as_str().rsplit('-').next().unwrap(), 16,
        ).unwrap();
        Ok(match id {
            2 => RegisteredBodyResult::BodyMismatch,
            3 => RegisteredBodyResult::MetadataMismatch,
            4 => RegisteredBodyResult::Missing,
            _ => RegisteredBodyResult::Verified,
        })
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 /tor_ci"]
async fn g09_body_sha_fenced_sample_restarts_and_distinguishes_unavailable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never access persistent PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query(
        "WITH ids AS (
             SELECT n, ('ff0b0000-0000-4000-8000-' ||
                        lpad(to_hex(n),12,'0'))::uuid AS oid
               FROM generate_series(1,18) AS x(n)
         )
         INSERT INTO storage_objects
            (id,object_kind,bucket,object_key,state,size_bytes,
             sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
             kek_id,object_nonce,object_aad,codec,verified_at,attached_at,created_at)
         SELECT oid,'chapter',$1,
                'private/chapter-versions/' || oid::text || '.enc',
                CASE WHEN n=5 THEN 'pending'
                     WHEN n IN (1,2,4) THEN 'attached' ELSE 'ready' END,
                CASE WHEN n=6 THEN 4194305 ELSE 64 END,
                decode(repeat('0a',32),'hex'),decode(repeat('0b',32),'hex'),
                decode(repeat('12',48),'hex'),decode(repeat('13',12),'hex'),
                'content-kek-v1',decode(repeat('14',12),'hex'),
                decode(repeat('15',64),'hex'),'gzip',
                CASE WHEN n=5 THEN NULL ELSE clock_timestamp() END,
                CASE WHEN n IN (1,2,4) THEN clock_timestamp() ELSE NULL END,
                clock_timestamp()-interval '48 hours'
           FROM ids",
    )
    .bind(BODY_BUCKET)
    .execute(&pool)
    .await
    .unwrap();

    let audit = PgBodyAudit::new(pool.clone());
    let mock = FakeBody {
        fail_remaining: AtomicUsize::new(1),
        probes: AtomicUsize::new(0),
    };
    assert_eq!(
        audit.scan_page(&mock, PrivateScope::Chapter).await.unwrap_err(),
        InventoryError::Unavailable
    );
    let unknown = audit.progress(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert_eq!((unknown.scanned,unknown.missing,unknown.unknown_attempts), (0,0,1));

    sqlx::query(
        "UPDATE g09_body_sha_scans
            SET leased_until=clock_timestamp()+interval '1 hour'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(BODY_BUCKET)
    .execute(&pool)
    .await.unwrap();
    assert!(audit.scan_page(&mock, PrivateScope::Chapter).await.unwrap().is_none());
    sqlx::query(
        "UPDATE g09_body_sha_scans
            SET leased_until=clock_timestamp()-interval '1 second'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(BODY_BUCKET)
    .execute(&pool)
    .await.unwrap();

    let restarted = PgBodyAudit::new(pool.clone());
    let mut pages = 0;
    while let Some(progress) = restarted.scan_page(&mock, PrivateScope::Chapter)
        .await.unwrap()
    {
        pages += 1;
        assert!(pages <= 3, "must advance PG cursor without repeating GETs");
        if progress.completed { break; }
    }
    let done = audit.progress(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert_eq!(pages, 3);
    assert_eq!(done.expected, 18);
    assert_eq!(done.scanned, 18);
    assert_eq!(done.verified, 13);
    assert_eq!(done.skipped, 2);
    assert_eq!(done.body_mismatch, 1);
    assert_eq!(done.metadata_mismatch, 1);
    assert_eq!(done.missing, 1);
    assert_eq!(done.unknown_attempts, 1);
    assert_eq!(done.retries, 1);
    assert!(done.completed);
    assert!(!done.exhausted);
    assert_eq!(mock.probes.load(Ordering::SeqCst), 17);
    assert!(audit.scan_page(&mock, PrivateScope::Chapter).await.unwrap().is_none());

    let findings: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM g09_body_sha_findings
          WHERE bucket=$1 AND scope='chapter' AND cycle=1",
    )
    .bind(BODY_BUCKET).fetch_one(&pool).await.unwrap();
    assert_eq!(findings, 3);
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM g09_body_sha_findings
          WHERE bucket=$1 AND scope='chapter' ORDER BY status",
    )
    .bind(BODY_BUCKET).fetch_all(&pool).await.unwrap();
    assert_eq!(statuses, ["body_mismatch", "metadata_mismatch", "missing"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM storage_objects WHERE bucket=$1",
        ).bind(BODY_BUCKET).fetch_one(&pool).await.unwrap(),
        18,
        "diagnostic process cannot mutate source storage objects"
    );

    assert!(audit.begin_next_cycle(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap());
    let next = audit.progress(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert_eq!((next.cycle, next.scanned, next.expected), (2, 0, 18));

    mock.fail_remaining.store(8, Ordering::SeqCst);
    for _ in 0..8 {
        assert_eq!(
            audit.scan_page(&mock, PrivateScope::Chapter).await.unwrap_err(),
            InventoryError::Unavailable
        );
    }
    let failed = audit.progress(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert!(failed.exhausted);
    assert_eq!(failed.scanned, 0);
    assert_eq!(failed.missing, 0);
    assert_eq!(failed.unknown_attempts, 8);
    assert!(audit.scan_page(&mock, PrivateScope::Chapter).await.unwrap().is_none());
    assert!(audit.resume_exhausted(BODY_BUCKET, PrivateScope::Chapter)
        .await.unwrap());
    let resumed = audit.scan_page(&mock, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert_eq!(resumed.scanned, 8);
    assert!(!resumed.exhausted);
    // Findings from previous completed cycles remain available for review.
    let retained: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM g09_body_sha_findings
          WHERE bucket=$1 AND cycle=1",
    ).bind(BODY_BUCKET).fetch_one(&pool).await.unwrap();
    assert_eq!(retained, 3);
    pool.close().await;
}
