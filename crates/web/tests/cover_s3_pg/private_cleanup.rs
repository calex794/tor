//! Isolated real PostgreSQL and MinIO checks for retired private cover WebP.
use aws_sdk_s3::Client;
use std::sync::Arc;
use tor_books::repositories::{
    cover_private_cleanup::PgPrivateCoverVariantRemovals, cover_slots::PgCoverSlots, PgCatalog,
};
use tor_ingest::{
    repositories::{cover_jobs::PgCoverJobs, s3::S3CoverStore},
    services::cover_jobs::{CoverJobs, CoverObjectStore},
};
use tor_web::cover_private_cleanup::PrivateCoverVariantCleanup;

use super::{digest, pg_sql, BOOK_ID, BUCKET, SOURCE_KEY};

pub(super) struct PrivateRetentionCase<'a> {
    pub url: &'a str,
    pub catalog: &'a PgCatalog,
    pub slots: &'a PgCoverSlots,
    pub jobs: &'a PgCoverJobs,
    pub store: S3CoverStore,
    pub client: &'a Client,
    pub source: i64,
    pub reviewer: i64,
    pub job: i64,
}

pub(super) async fn verify_private_variant_retention(case: PrivateRetentionCase<'_>) {
    let PrivateRetentionCase {
        url,
        catalog,
        slots,
        jobs,
        store,
        client,
        source,
        reviewer,
        job,
    } = case;
    let url = url.to_owned();
    let stored_variants = jobs.existing_variants(job).await.unwrap();
    // MOD-COVER: old rejected private WebP variants may be removed, but an
    // active approved slot, its original upload, and immutable audit MUST remain.
    let private_cleanup = PrivateCoverVariantCleanup::new(
        PgPrivateCoverVariantRemovals::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    assert_eq!(private_cleanup.run_batch().await.unwrap().claimed, 0);
    assert!(store
        .remove_private_staging_variant(SOURCE_KEY)
        .await
        .is_err());
    assert!(store
        .remove_private_staging_variant("staging/covers/variants/../escape.webp")
        .await
        .is_err());
    let retired_job = jobs
        .enqueue(source, BOOK_ID, BOOK_ID, 1, "test-retention-20261107")
        .await
        .unwrap();
    pg_sql(
        &url,
        &format!("UPDATE cover_variant_jobs SET status='done' WHERE id={retired_job}"),
    )
    .await;
    let mut retired_keys = Vec::new();
    for variant in &stored_variants {
        let key = format!(
            "staging/covers/variants/{retired_job}/retired-{}.webp",
            variant.name
        );
        let bytes = store
            .get_bounded(&variant.storage_key, variant.size_bytes as usize)
            .await
            .unwrap();
        store.put_verified(&key, &bytes).await.unwrap();
        pg_sql(
            &url,
            &format!(
                "INSERT INTO cover_variants
                 (job_id,variant_name,storage_key,content_sha256,size_bytes)
                 VALUES({retired_job},'{}','{}',decode('{}','hex'),{})",
                variant.name,
                key,
                hex::encode(digest(&bytes)),
                bytes.len()
            ),
        )
        .await;
        retired_keys.push(key);
    }
    let retired_review = pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_requests
             (book_id,job_id,submitted_by_user_id,expected_slot_revision,
              state,submitted_at,reviewer_user_id,decided_at,reason)
             VALUES({BOOK_ID},{retired_job},{BOOK_ID},1,'rejected',
                    now()-interval '9 days',{reviewer},
                    now()-interval '8 days','Declined by reviewer')
             RETURNING id"
        ),
    )
    .await;
    let retired_review_id: i64 = retired_review.parse().unwrap();
    pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_decisions
             (request_id,book_id,job_id,reviewer_user_id,outcome,reason)
             VALUES({retired_review_id},{BOOK_ID},{retired_job},
                    {reviewer},'rejected','Declined by reviewer')"
        ),
    )
    .await;
    let cleanup = private_cleanup.run_batch().await.unwrap();
    assert_eq!(cleanup.claimed, 2);
    assert_eq!(cleanup.removed, 2);
    assert_eq!(cleanup.deferred, 0);
    assert_eq!(private_cleanup.run_batch().await.unwrap().claimed, 0);
    for key in &retired_keys {
        assert!(
            client
                .head_object()
                .bucket(BUCKET)
                .key(key)
                .send()
                .await
                .is_err(),
            "rejected private variant remains in MinIO"
        );
    }
    assert_eq!(
        pg_sql(
            &url,
            "SELECT count(*) FROM cover_private_variant_removals WHERE state='removed'",
        )
        .await,
        "2",
        "retention records must be durable and idempotent"
    );
    assert_eq!(
        pg_sql(
            &url,
            &format!("SELECT count(*) FROM book_cover_review_decisions WHERE job_id={retired_job}"),
        )
        .await,
        "1",
        "private S3 removal must not delete reviewer audit"
    );
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 1)));
    assert!(
        client
            .head_object()
            .bucket(BUCKET)
            .key(SOURCE_KEY)
            .send()
            .await
            .is_ok(),
        "a source reused by approved content must never be deleted"
    );
    for variant in &stored_variants {
        assert!(store
            .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
            .await
            .unwrap());
    }

    // A candidate submitted twenty days ago but superseded today must
    // NOT lose its private files until seven full days after supersession.
    // Historical rows without superseded_at are deliberately fail-closed.
    let superseded_job = jobs
        .enqueue(
            source,
            BOOK_ID,
            BOOK_ID,
            1,
            "test-superseded-retention-20261107",
        )
        .await
        .unwrap();
    pg_sql(
        &url,
        &format!("UPDATE cover_variant_jobs SET status='done' WHERE id={superseded_job}"),
    )
    .await;
    let mut superseded_keys = Vec::new();
    for variant in &stored_variants {
        let key = format!(
            "staging/covers/variants/{superseded_job}/superseded-{}.webp",
            variant.name
        );
        let bytes = store
            .get_bounded(&variant.storage_key, variant.size_bytes as usize)
            .await
            .unwrap();
        store.put_verified(&key, &bytes).await.unwrap();
        pg_sql(
            &url,
            &format!(
                "INSERT INTO cover_variants
                 (job_id,variant_name,storage_key,content_sha256,size_bytes)
                 VALUES({superseded_job},'{}','{}',decode('{}','hex'),{})",
                variant.name,
                key,
                hex::encode(digest(&bytes)),
                bytes.len()
            ),
        )
        .await;
        superseded_keys.push(key);
    }
    pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_requests
             (book_id,job_id,submitted_by_user_id,expected_slot_revision,
              state,submitted_at)
             VALUES({BOOK_ID},{superseded_job},{BOOK_ID},1,'superseded',
                    now()-interval '20 days')"
        ),
    )
    .await;
    assert_eq!(
        private_cleanup.run_batch().await.unwrap().claimed,
        0,
        "a legacy superseded request without terminal timestamp must not be deleted"
    );
    pg_sql(
        &url,
        &format!(
            "UPDATE book_cover_review_requests SET superseded_at=now()-interval '1 day'
             WHERE job_id={superseded_job}"
        ),
    )
    .await;
    assert_eq!(
        private_cleanup.run_batch().await.unwrap().claimed,
        0,
        "submission age must not substitute for seven days since supersession"
    );
    for key in &superseded_keys {
        assert!(
            client
                .head_object()
                .bucket(BUCKET)
                .key(key)
                .send()
                .await
                .is_ok(),
            "recently superseded private candidate must remain in MinIO"
        );
    }
    pg_sql(
        &url,
        &format!(
            "UPDATE book_cover_review_requests SET superseded_at=now()-interval '8 days'
             WHERE job_id={superseded_job}"
        ),
    )
    .await;
    let aged = private_cleanup.run_batch().await.unwrap();
    assert_eq!(aged.claimed, 2);
    assert_eq!(aged.removed, 2);
    assert_eq!(aged.deferred, 0);
    for key in &superseded_keys {
        assert!(
            client
                .head_object()
                .bucket(BUCKET)
                .key(key)
                .send()
                .await
                .is_err(),
            "only sufficiently aged superseded private variants may be removed"
        );
    }
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 1)));
    for variant in &stored_variants {
        assert!(store
            .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
            .await
            .unwrap());
    }
}
