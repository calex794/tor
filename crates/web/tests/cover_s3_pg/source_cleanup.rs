//! Real MinIO + PostgreSQL proof that one original may serve multiple jobs.
use std::sync::Arc;
use tor_books::repositories::{
    cover_private_cleanup::PgPrivateCoverVariantRemovals,
    cover_source_cleanup::PgPrivateCoverSourceRemovals,
};
use tor_ingest::{
    domain::cover::CoverVariant,
    repositories::s3::S3CoverStore,
    services::cover_jobs::{CoverJobs, CoverObjectStore},
};
use tor_web::{
    cover_private_cleanup::PrivateCoverVariantCleanup,
    cover_private_source_cleanup::PrivateCoverSourceCleanup,
};

use super::{
    digest, pg_sql, private_cleanup::PrivateRetentionCase, source_png, BOOK_ID, BUCKET, SOURCE_KEY,
};

const RETIRED_SOURCE: &str = "staging/covers/sources/obs09-retired-shared-source-950.png";

async fn stage_pair(
    job: i64,
    tag: &str,
    originals: &[CoverVariant],
    store: &S3CoverStore,
    url: &str,
) -> Vec<String> {
    let mut keys = Vec::new();
    for variant in originals {
        let key = format!(
            "staging/covers/variants/{job}/retired-{tag}-{}.webp",
            variant.name
        );
        let bytes = store
            .get_bounded(&variant.storage_key, variant.size_bytes as usize)
            .await
            .unwrap();
        store.put_verified(&key, &bytes).await.unwrap();
        pg_sql(
            url,
            &format!(
                "INSERT INTO cover_variants
                 (job_id,variant_name,storage_key,content_sha256,size_bytes)
                 VALUES({job},'{}','{}',decode('{}','hex'),{})",
                variant.name,
                key,
                hex::encode(digest(&bytes)),
                bytes.len()
            ),
        )
        .await;
        keys.push(key);
    }
    keys
}

pub(super) async fn verify_retired_shared_source(case: PrivateRetentionCase<'_>) {
    let PrivateRetentionCase {
        url,
        catalog,
        slots,
        jobs,
        store,
        client,
        source: _,
        reviewer,
        job,
    } = case;
    let url = url.to_owned();
    let originals = jobs.existing_variants(job).await.unwrap();
    let bytes = source_png();
    let hash = digest(&bytes);
    store
        .put_source_verified(RETIRED_SOURCE, &bytes, "image/png", &hash)
        .await
        .unwrap();
    let shared_source = jobs
        .register_verified_source(
            BOOK_ID,
            BOOK_ID,
            RETIRED_SOURCE,
            "image/png",
            bytes.len() as i64,
            &hash,
        )
        .await
        .unwrap();
    let rejected_job = jobs
        .enqueue(
            shared_source,
            BOOK_ID,
            BOOK_ID,
            1,
            "retired-shared-source-rejected",
        )
        .await
        .unwrap();
    let pending_job = jobs
        .enqueue(
            shared_source,
            BOOK_ID,
            BOOK_ID,
            1,
            "retired-shared-source-pending",
        )
        .await
        .unwrap();
    assert_ne!(rejected_job, pending_job);
    assert_eq!(
        jobs.enqueue(
            shared_source,
            BOOK_ID,
            BOOK_ID,
            1,
            "retired-shared-source-rejected"
        )
        .await
        .unwrap(),
        rejected_job,
        "a verified source permits idempotent replay of an existing recipe"
    );
    pg_sql(
        &url,
        &format!("UPDATE cover_variant_jobs SET status='done' WHERE id={rejected_job}"),
    )
    .await;
    let rejected_keys = stage_pair(rejected_job, "reject", &originals, &store, &url).await;
    let review_id = pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_requests
             (book_id,job_id,submitted_by_user_id,expected_slot_revision,
              state,submitted_at,reviewer_user_id,decided_at,reason)
             VALUES({BOOK_ID},{rejected_job},{BOOK_ID},1,'rejected',
                    now()-interval '11 days',{reviewer},
                    now()-interval '9 days','Wrong cover')
             RETURNING id"
        ),
    )
    .await;
    let review_id: i64 = review_id.parse().unwrap();
    pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_decisions
             (request_id,book_id,job_id,reviewer_user_id,outcome,reason)
             VALUES({review_id},{BOOK_ID},{rejected_job},
                    {reviewer},'rejected','Wrong cover')"
        ),
    )
    .await;

    let variant_cleanup = PrivateCoverVariantCleanup::new(
        PgPrivateCoverVariantRemovals::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    let source_cleanup = PrivateCoverSourceCleanup::new(
        PgPrivateCoverSourceRemovals::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    assert_eq!(variant_cleanup.run_batch().await.unwrap().removed, 2);
    assert_eq!(source_cleanup.run_batch().await.unwrap().claimed, 0);
    assert!(
        client
            .head_object()
            .bucket(BUCKET)
            .key(RETIRED_SOURCE)
            .send()
            .await
            .is_ok(),
        "a source shared with an unfinished recipe must survive first rejection"
    );
    for key in &rejected_keys {
        assert!(client
            .head_object()
            .bucket(BUCKET)
            .key(key)
            .send()
            .await
            .is_err());
    }

    pg_sql(
        &url,
        &format!("UPDATE cover_variant_jobs SET status='done' WHERE id={pending_job}"),
    )
    .await;
    let pending_keys = stage_pair(pending_job, "superseded", &originals, &store, &url).await;
    pg_sql(
        &url,
        &format!(
            "INSERT INTO book_cover_review_requests
             (book_id,job_id,submitted_by_user_id,expected_slot_revision,
              state,submitted_at,superseded_at)
             VALUES({BOOK_ID},{pending_job},{BOOK_ID},1,'superseded',
                    now()-interval '20 days',now()-interval '8 days')"
        ),
    )
    .await;
    assert_eq!(source_cleanup.run_batch().await.unwrap().claimed, 0);
    assert_eq!(variant_cleanup.run_batch().await.unwrap().removed, 2);
    let deleted = source_cleanup.run_batch().await.unwrap();
    assert_eq!(deleted.claimed, 1);
    assert_eq!(deleted.removed, 1);
    assert_eq!(deleted.deferred, 0);
    assert_eq!(source_cleanup.run_batch().await.unwrap().claimed, 0);
    for key in &pending_keys {
        assert!(client
            .head_object()
            .bucket(BUCKET)
            .key(key)
            .send()
            .await
            .is_err());
    }
    assert!(
        client
            .head_object()
            .bucket(BUCKET)
            .key(RETIRED_SOURCE)
            .send()
            .await
            .is_err(),
        "retired original remains in MinIO after all jobs have aged out"
    );
    assert_eq!(
        pg_sql(
            &url,
            &format!("SELECT status FROM cover_upload_objects WHERE id={shared_source}"),
        )
        .await,
        "retired"
    );
    assert_eq!(
        pg_sql(
            &url,
            "SELECT count(*) FROM cover_private_source_removals WHERE state='removed'"
        )
        .await,
        "1"
    );
    assert!(
        jobs.enqueue(
            shared_source,
            BOOK_ID,
            BOOK_ID,
            1,
            "retired-source-after-claim"
        )
        .await
        .is_err(),
        "new recipes must not be able to enqueue after source retirement"
    );
    assert!(
        jobs.enqueue(
            shared_source,
            BOOK_ID,
            BOOK_ID,
            1,
            "retired-shared-source-rejected"
        )
        .await
        .is_err(),
        "an already retired original must also reject old recipe replays"
    );
    assert!(store
        .remove_private_staging_source("staging/covers/variants/retired.webp")
        .await
        .is_err());
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 1)));
    assert!(client
        .head_object()
        .bucket(BUCKET)
        .key(SOURCE_KEY)
        .send()
        .await
        .is_ok());
    assert_eq!(
        pg_sql(
            &url,
            &format!(
                "SELECT count(*) FROM book_cover_review_decisions WHERE job_id={rejected_job}"
            )
        )
        .await,
        "1",
        "source removal must retain immutable moderator decisions"
    );
    for variant in &originals {
        assert!(store
            .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
            .await
            .unwrap());
    }
}
